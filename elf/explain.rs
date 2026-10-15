//! --why-kept and --why-removed: say why --gc-sections kept or removed a
//! named section, and why --icf folded it or left it alone.
//!
//! The garbage collector marks sections in parallel and remembers only
//! that a section was reached. To say by which path, we run a second,
//! single-threaded walk over the same graph before the real collection,
//! and write down how each section was first reached. The real collection
//! is untouched, so nothing here can change what is kept.

use std::collections::{HashMap, VecDeque};



use crate::arch::{Family, Target};
use crate::context::Context;
use crate::elf::*;
use crate::gc_sections::{StartStopMap, build_start_stop_map, should_keep, start_stop_name};
use crate::input_files::FileId;
use crate::input_sections::SectionRef;
use crate::symbol::OriginValue;

enum Why {
    /// Kept for a reason of its own, not because another section refers to it.
    Root(String),
    /// Reached through a reference from another section.
    Via { from: SectionRef, sym: String },
}

pub struct GcTrace {
    why: HashMap<SectionRef, Why>,
}

/// `*` matches any run of characters; everything else matches itself.
fn wildcard(pat: &[u8], s: &[u8]) -> bool {
    match pat.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|i| wildcard(rest, &s[i..])),
        Some((c, rest)) => s.first() == Some(c) && wildcard(rest, &s[1..]),
    }
}

fn why_root<E: Target>(
    file: &crate::input_files::ObjectFile<E>,
    isec: &crate::input_sections::InputSection<E>,
) -> String {
    let ty = isec.sh_type(file);
    let name: &[u8] = isec.name(file);
    if isec.sh_flags as u32 & SHF_GNU_RETAIN != 0 {
        "it is marked SHF_GNU_RETAIN".to_string()
    } else if matches!(ty, SHT_INIT_ARRAY | SHT_FINI_ARRAY | SHT_PREINIT_ARRAY) {
        "it is an init/fini array, which the program runs at startup".to_string()
    } else if ty == SHT_NOTE {
        "it is a note section".to_string()
    } else {
        format!("sections named like `{}` are never removed", String::from_utf8_lossy(name))
    }
}

/// Walks the reference graph as --gc-sections does and records how each
/// section was first reached.
pub fn trace_gc<E: Target>(ctx: &Context<E>) -> GcTrace {
    let mut why: HashMap<SectionRef, Why> = HashMap::new();
    let mut queue: VecDeque<SectionRef> = VecDeque::new();
    let map: StartStopMap<E> = build_start_stop_map(ctx);

    let add_root = |why: &mut HashMap<SectionRef, Why>,
                    queue: &mut VecDeque<SectionRef>,
                    r: SectionRef,
                    reason: String| {
        if !ctx.section(r).is_alive() {
            return;
        }
        if !why.contains_key(&r) {
            why.insert(r, Why::Root(reason));
            queue.push_back(r);
        }
    };

    for file in &ctx.objs {
        let file_id = FileId::Obj(file.id());
        for isec in file.input_sections() {
            if isec.is_alive() && isec.is_alloc() && should_keep::<E>(file, isec) {
                add_root(&mut why, &mut queue, isec.section_ref(), why_root(file, isec));
            }
        }
        for &id in &file.base.symbols {
            let sym = &ctx.symbols[id];
            if sym.file() != Some(file_id) {
                continue;
            }
            let reason = if sym.gc_root() {
                format!(
                    "it defines `{sym}`, which must stay (-u, -e, --require-defined and the like)"
                )
            } else if sym.is_exported() {
                format!("it defines `{sym}`, which goes in the dynamic symbol table")
            } else {
                continue;
            };
            if let OriginValue::InputSection(s) = sym.origin() {
                add_root(&mut why, &mut queue, ctx.input_section(s).section_ref(), reason);
            }
        }
        for cie in &file.cies {
            for rel in cie.rels(file) {
                let sym = &ctx.symbols[file.base.symbols[rel.r_sym() as usize]];
                if let OriginValue::InputSection(s) = sym.origin() {
                    let reason = format!(
                        "an .eh_frame CIE record of {file} refers to `{sym}`; the linker keeps all CIEs"
                    );
                    add_root(&mut why, &mut queue, ctx.input_section(s).section_ref(), reason);
                }
            }
        }
    }

    while let Some(r) = queue.pop_front() {
        let isec = ctx.section(r);
        let file = &ctx.objs[r.file.index()];
        let reach = |why: &mut HashMap<SectionRef, Why>,
                     queue: &mut VecDeque<SectionRef>,
                     target: SectionRef,
                     sym: String| {
            if ctx.section(target).is_alive() && !why.contains_key(&target) {
                why.insert(target, Why::Via { from: r, sym });
                queue.push_back(target);
            }
        };
        for fde in isec.fdes(file) {
            for rel in fde.rels(file).iter().skip(1) {
                let sym = &ctx.symbols[file.base.symbols[rel.r_sym() as usize]];
                if let Some(t) = sym.input_section_ref(ctx) {
                    reach(&mut why, &mut queue, t.section_ref(), format!("{sym} (unwind info)"));
                }
            }
        }
        for rel in isec.rels(file) {
            let sym = &ctx.symbols[file.base.symbols[rel.r_sym() as usize]];
            if let Some(FileId::Dso(_)) = sym.file() {
                continue;
            }
            if let OriginValue::InputSection(s) = sym.origin() {
                reach(&mut why, &mut queue, ctx.input_section(s).section_ref(), sym.to_string());
            }
            if let Some(name) = start_stop_name(sym.name())
                && let Some(targets) = map.get(name)
            {
                for t in targets {
                    reach(&mut why, &mut queue, t.section_ref(), sym.to_string());
                }
            }
        }
        if E::FAMILY == Family::Arm32
            && let Some(exidx) = isec.exidx()
        {
            reach(
                &mut why,
                &mut queue,
                file.section_at(exidx).section_ref(),
                "exception index".into(),
            );
        }
    }
    GcTrace { why }
}

/// The sections a --why-kept name stands for: symbols first, then
/// section names.
fn resolve<E: Target>(ctx: &Context<E>, pat: &str) -> Vec<SectionRef> {
    let pat = pat.as_bytes();
    let mut out = Vec::new();
    for id in ctx.symbols.global_ids() {
        let sym = &ctx.symbols[id];
        if wildcard(pat, sym.name())
            && let Some(isec) = sym.input_section_ref(ctx)
        {
            out.push(isec.section_ref());
        }
    }
    if out.is_empty() {
        for file in &ctx.objs {
            for isec in file.input_sections() {
                if wildcard(pat, isec.name(file)) {
                    out.push(isec.section_ref());
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out.truncate(20);
    out
}

struct Answer {
    query: String,
    section: String,
    /// `kept`, `removed`, `folded`, `not-folded` or `unknown`.
    decision: &'static str,
    by: &'static str,
    /// The whole explanation, one sentence per line (the text output, and
    /// the JSON `chain` at `full`).
    lines: Vec<String>,
    /// The same without the words that repeat the section's name or only
    /// explain how the linker works, for the JSON below `full`.
    short: Vec<String>,
}

/// The longest chain shown below `full`: the first hops and the last ones
/// (the root), with a marker for the hops between.
const CHAIN_HEAD: usize = 4;
const CHAIN_TAIL: usize = 3;

fn emit(a: &Answer) {
    use mold_common::out;
    if mold_common::error::json_enabled() {
        use mold_common::error::DetailLevel;
        let level = mold_common::error::detail_level();
        let mut o = mold_common::error::JsonObj::new("why");
        o.str("query", &a.query).str("section", &a.section);
        if level == DetailLevel::Full {
            o.str("by", a.by).str("decision", a.decision).strs("chain", &a.lines);
        } else if level == DetailLevel::Brief {
            o.str("decision", a.decision);
            if let Some(first) = a.short.first() {
                o.str("reason", first);
            }
            if a.short.len() > 1 {
                o.num("chain_length", a.short.len() as u64);
            }
        } else {
            o.str("by", a.by).str("decision", a.decision);
            if a.short.len() > CHAIN_HEAD + CHAIN_TAIL + 1 {
                let hidden = a.short.len() - CHAIN_HEAD - CHAIN_TAIL;
                let mut chain: Vec<String> = a.short[..CHAIN_HEAD].to_vec();
                chain.extend_from_slice(&a.short[a.short.len() - CHAIN_TAIL..]);
                o.strs("chain", &chain).num("chain_omitted", hidden as u64);
            } else {
                o.strs("chain", &a.short);
            }
        }
        mold_common::error::out_record("why", &o.finish());
    } else {
        out!("{}: {} by {}", a.section, a.decision, a.by);
        for l in &a.lines {
            out!("  {l}");
        }
    }
}

/// Answers each --why-kept name after the garbage collection has run.
pub fn report_gc<E: Target>(ctx: &Context<E>, trace: &GcTrace) {
    for query in &ctx.args.why_kept {
        let found = resolve(ctx, query);
        if found.is_empty() {
            emit(&Answer {
                query: query.clone(),
                section: query.clone(),
                decision: "unknown",
                by: "--gc-sections",
                lines: vec!["no symbol or input section has this name".into()],
                short: vec!["no symbol or input section has this name".into()],
            });
        }
        for r in found {
            let isec = ctx.section(r);
            let section = ctx.section_display(r).to_string();
            if isec.is_alive() {
                let mut lines = Vec::new();
                let mut short = Vec::new();
                let mut cur = r;
                for _ in 0..10000 {
                    match trace.why.get(&cur) {
                        Some(Why::Via { from, sym }) => {
                            lines.push(format!(
                                "{} is referenced via `{sym}` from {}",
                                ctx.section_display(cur),
                                ctx.section_display(*from)
                            ));
                            short.push(format!(
                                "via `{sym}` from {}",
                                ctx.section_display(*from)
                            ));
                            cur = *from;
                        }
                        Some(Why::Root(reason)) => {
                            lines.push(format!("{} is a root: {reason}", ctx.section_display(cur)));
                            short.push(format!("root: {reason}"));
                            break;
                        }
                        None => {
                            lines.push(format!(
                                "{} is not subject to garbage collection (not allocated in memory)",
                                ctx.section_display(cur)
                            ));
                            short.push(
                                "not subject to garbage collection (not allocated in memory)"
                                    .into(),
                            );
                            break;
                        }
                    }
                }
                emit(&Answer {
                    query: query.clone(),
                    section,
                    decision: "kept",
                    by: "--gc-sections",
                    lines,
                    short,
                });
            } else {
                let mut lines = vec!["no section that is kept refers to it".to_string()];
                // The reader asked why it is kept (--why-kept) and it is
                // not even in the output: say so first.
                let asked_kept = !ctx.args.why_removed.contains(query);
                let mut short = vec![if asked_kept {
                    "it is not in the output: no section that is kept refers to it".to_string()
                } else {
                    "no section that is kept refers to it".to_string()
                }];
                let mut n = 0;
                let json = mold_common::error::json_enabled();
                let mut debug_count = 0;
                'outer: for file in &ctx.objs {
                    for from in file.input_sections() {
                        // Unwind info for a section is not a use of it.
                        if from.name(file) == b".eh_frame" {
                            continue;
                        }
                        // In JSON (agent) output the section's own debug
                        // sections are not reasons; they are counted.
                        let own_debug = json
                            && from.section_ref().file == r.file
                            && from.name(file).starts_with(b".debug_");
                        for rel in from.rels(file) {
                            let sym = &ctx.symbols[file.base.symbols[rel.r_sym() as usize]];
                            if let OriginValue::InputSection(s) = sym.origin()
                                && ctx.input_section(s).section_ref() == r
                                && from.section_ref() != r
                            {
                                if own_debug {
                                    debug_count += 1;
                                    break;
                                }
                                lines.push(format!(
                                    "referenced via `{sym}` from {}, which is removed too",
                                    ctx.section_display(from.section_ref())
                                ));
                                short.push(format!(
                                    "referenced via `{sym}` from {}, which is removed too",
                                    ctx.section_display(from.section_ref())
                                ));
                                n += 1;
                                if n == 5 {
                                    break 'outer;
                                }
                                break;
                            }
                        }
                    }
                }
                if debug_count > 0 {
                    let text = if debug_count == 1 {
                        "1 debug section of the same object is also removed".to_string()
                    } else {
                        format!("{debug_count} debug sections of the same object are also removed")
                    };
                    lines.push(text.clone());
                    short.push(text);
                }
                emit(&Answer {
                    query: query.clone(),
                    section,
                    decision: "removed",
                    by: "--gc-sections",
                    lines,
                    short,
                });
            }
        }
    }
}

/// Answers each --why-kept name after --icf has chosen leaders. `sections`
/// are the sections ICF looked at.
pub fn report_icf<E: Target>(ctx: &Context<E>, sections: &[SectionRef]) {
    let looked: std::collections::HashSet<SectionRef> = sections.iter().copied().collect();
    for query in &ctx.args.why_kept {
        for r in resolve(ctx, query) {
            let isec = ctx.section(r);
            if !isec.is_alive() {
                continue;
            }
            let section = ctx.section_display(r).to_string();
            let file = &ctx.objs[r.file.index()];
            let mut lines = Vec::new();
            // Below `full` the two sentences that explain ICF in general go.
            let mut generic = 0;
            let decision;
            if !looked.contains(&r) {
                decision = "not-folded";
                lines.push(if crate::icf::is_eligible(ctx, isec) {
                    "it was not considered".to_string()
                } else {
                    let name: &[u8] = isec.name(file);
                    let why = if isec.sh_size == 0 {
                        "it is empty"
                    } else if !isec.is_alloc() {
                        "it is not loaded into memory"
                    } else if isec.sh_type(file) == SHT_NOBITS {
                        "it has no contents (.bss)"
                    } else if crate::symbol::is_c_identifier(name) {
                        "its name can be used with __start_/__stop_ symbols"
                    } else if isec.sh_flags & SHF_EXECINSTR as u64 != 0 {
                        "it is .init or .fini"
                    } else {
                        "it is writable data, whose copies must stay separate"
                    };
                    format!("ICF only folds read-only data and code; {why}")
                });
            } else {
                let leader = isec.icf_leader_in_round();
                if leader != r {
                    decision = "folded";
                    lines.push(format!("folded into {}", ctx.section_display(leader)));
                    lines.push(
                        "both have identical contents, and their relocations point to sections \
                         that are themselves identical"
                            .into(),
                    );
                    lines.push(
                        "the section that comes first in link order is the one that stays".into(),
                    );
                    generic = 2;
                } else {
                    decision = "not-folded";
                    let folded: Vec<String> = sections
                        .iter()
                        .filter(|&&o| o != r && ctx.section(o).icf_leader_in_round() == r)
                        .take(5)
                        .map(|&o| ctx.section_display(o).to_string())
                        .collect();
                    if !folded.is_empty() {
                        lines.push(format!(
                            "it is the one that stays; folded into it: {}",
                            folded.join(", ")
                        ));
                    } else if crate::icf::needs_unique_address(ctx, isec) {
                        lines.push(
                            "its address is taken, so another section may compare pointers to it; \
                             --icf=all (code) or --ignore-data-address-equality (data) would let it fold"
                                .into(),
                        );
                    } else {
                        lines.push(
                            "no other section has the same contents and relocation targets".into(),
                        );
                    }
                }
            }
            // The generic sentences are the last two of a "folded" answer.
            let mut short = lines.clone();
            short.truncate(lines.len() - generic);
            emit(&Answer { query: query.clone(), section, decision, by: "--icf", lines, short });
        }
    }
}
