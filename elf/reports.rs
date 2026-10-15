//! Reports about a finished link that people and programs ask for with
//! --size-report and --link-summary. They only read what the link produced.

use std::collections::HashMap;
use std::fmt::Write as _;

use mold_common::error::{DetailLevel, JsonObj, json_string};

use crate::arch::Target;
use crate::chunks::ChunkId;
use crate::cmdline::ReportOutput;
use crate::context::Context;
use crate::elf::*;

struct SectionRow {
    name: String,
    size: u64,
    alloc: bool,
    inputs: usize,
}

#[derive(Default)]
struct FileRow {
    name: String,
    size: u64,
    /// Bytes per output section, largest first.
    sections: Vec<(String, u64)>,
}

fn collect<E: Target>(ctx: &Context<E>) -> (Vec<SectionRow>, Vec<FileRow>) {
    let mut sections = Vec::new();
    let mut files: HashMap<usize, FileRow> = HashMap::new();
    for &id in &ctx.chunks {
        let hdr = ctx.chunk_header(id);
        // Chunks that do not occupy the output file (the section header
        // table has size, the null section does not) are still listed; a
        // NOBITS section has a size in memory only.
        let name = hdr.name.to_string();
        if name.is_empty() {
            continue;
        }
        let mut inputs = 0;
        if let ChunkId::Output(osec_id) = id {
            let osec = &ctx.output_sections[osec_id.index()];
            inputs = osec.members.len();
            let mut per_file: HashMap<usize, u64> = HashMap::new();
            for &member in &osec.members {
                let isec = ctx.input_section(member);
                *per_file.entry(isec.file.index()).or_default() += isec.sh_size;
            }
            for (file, size) in per_file {
                let row = files.entry(file).or_insert_with(|| FileRow {
                    name: ctx.objs[file].to_string(),
                    ..Default::default()
                });
                row.size += size;
                row.sections.push((name.clone(), size));
            }
        }
        sections.push(SectionRow {
            name,
            size: hdr.shdr.sh_size.get(),
            alloc: hdr.is_alloc(),
            inputs,
        });
    }
    // Rank. Ties are broken by name so that the report is stable.
    sections.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    let mut files: Vec<FileRow> = files.into_values().collect();
    for f in &mut files {
        f.sections.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    }
    files.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    (sections, files)
}

fn output_file_size<E: Target>(ctx: &Context<E>) -> Option<u64> {
    std::fs::metadata(&ctx.args.output).ok().map(|m| m.len())
}

/// --size-report: where the bytes of the output came from.
pub fn size_report<E: Target>(ctx: &Context<E>, output: &ReportOutput) {
    let _t = ctx.timer("size_report");
    let (sections, files) = collect(ctx);
    let file_size = output_file_size(ctx);
    let json = match output {
        ReportOutput::File(p) => p.extension().is_some_and(|e| e == "json"),
        ReportOutput::Stdout => mold_common::error::json_enabled(),
    };
    let mut text = String::new();
    if json {
        let level = mold_common::error::detail_level();
        let full = level == DetailLevel::Full;
        let on_stdout = matches!(output, ReportOutput::Stdout);
        let mut o = JsonObj::new("size-report");
        o.str("output", &ctx.args.output.display().to_string());
        match file_size {
            Some(n) => {
                o.num("output_size", n);
            }
            None if full => {
                o.null("output_size");
            }
            None => {}
        }
        // Below full a section with nothing in it is left out, and the
        // lists on standard output are cut to the largest; a file gets
        // every entry.
        let (sec_cap, file_cap, per_file) = match (level, on_stdout) {
            (DetailLevel::Full, _) | (_, false) => (usize::MAX, usize::MAX, usize::MAX),
            (DetailLevel::Normal, true) => (12, 8, 3),
            (DetailLevel::Brief, true) => (5, 5, 0),
        };
        let secs: Vec<&SectionRow> =
            sections.iter().filter(|s| full || s.size > 0).collect();
        let mut list = String::from("[");
        for (i, s) in secs.iter().take(sec_cap).enumerate() {
            if i > 0 {
                list.push(',');
            }
            list.push_str("{\"name\":");
            json_string(&mut list, &s.name);
            write!(list, ",\"size\":{}", s.size).unwrap();
            if full || s.alloc {
                write!(list, ",\"alloc\":{}", s.alloc).unwrap();
            }
            if full || s.inputs > 0 {
                write!(list, ",\"inputs\":{}", s.inputs).unwrap();
            }
            list.push('}');
        }
        list.push(']');
        o.raw("sections", &list);
        if !full && secs.len() > sec_cap {
            let rest: u64 = secs[sec_cap..].iter().map(|s| s.size).sum();
            o.num("sections_omitted", (secs.len() - sec_cap) as u64);
            o.num("sections_omitted_size", rest);
        }
        // On standard output (where a program reads it) only the largest
        // files are listed, as in the text report; a file gets them all.
        let shown = match output {
            ReportOutput::Stdout => files.len().min(if full { 30 } else { file_cap }),
            ReportOutput::File(_) => files.len(),
        };
        let omitted: u64 = files[shown..].iter().map(|f| f.size).sum();
        let mut list = String::from("[");
        for (i, f) in files[..shown].iter().enumerate() {
            if i > 0 {
                list.push(',');
            }
            list.push_str("{\"file\":");
            json_string(&mut list, &f.name);
            write!(list, ",\"size\":{}", f.size).unwrap();
            let parts: Vec<&(String, u64)> =
                f.sections.iter().filter(|(_, n)| full || *n > 0).collect();
            if full {
                list.push_str(",\"sections\":[");
            } else if per_file > 0 && !parts.is_empty() {
                list.push_str(",\"sections\":[");
            }
            if full || (per_file > 0 && !parts.is_empty()) {
                for (j, (name, size)) in parts.iter().take(per_file).enumerate() {
                    if j > 0 {
                        list.push(',');
                    }
                    list.push_str("{\"name\":");
                    json_string(&mut list, name);
                    write!(list, ",\"size\":{size}}}").unwrap();
                }
                list.push(']');
            }
            if !full && parts.len() > per_file && per_file > 0 {
                write!(list, ",\"sections_omitted\":{}", parts.len() - per_file).unwrap();
            }
            list.push('}');
        }
        list.push(']');
        o.raw("files", &list);
        if full || files.len() > shown {
            o.num("files_omitted", (files.len() - shown) as u64);
            o.num("omitted_size", omitted);
        }
        if !full && on_stdout && (secs.len() > sec_cap || files.len() > shown) {
            o.str("more", "--size-report=FILE.json lists every file and section");
        }
        text = o.finish();
        text.push('\n');
    } else {
        match file_size {
            Some(n) => writeln!(text, "Output file: {} ({n} bytes)", ctx.args.output.display()),
            None => writeln!(text, "Output file: {}", ctx.args.output.display()),
        }
        .unwrap();
        writeln!(text, "\nBy output section (bytes in the link, largest first):").unwrap();
        for s in sections.iter().filter(|s| s.size > 0) {
            if s.inputs > 0 {
                writeln!(text, "{:>12}  {}  ({} input sections)", s.size, s.name, s.inputs)
                    .unwrap();
            } else {
                writeln!(text, "{:>12}  {}  (made by the linker)", s.size, s.name).unwrap();
            }
        }
        const TOP: usize = 30;
        writeln!(text, "\nBy input file (bytes of input sections kept, largest first):").unwrap();
        for f in files.iter().take(TOP) {
            let parts: Vec<String> =
                f.sections.iter().take(3).map(|(n, s)| format!("{n} {s}")).collect();
            writeln!(text, "{:>12}  {}  [{}]", f.size, f.name, parts.join(", ")).unwrap();
        }
        if files.len() > TOP {
            let rest: u64 = files[TOP..].iter().map(|f| f.size).sum();
            writeln!(text, "{rest:>12}  ({} more files)", files.len() - TOP).unwrap();
        }
    }
    if json && matches!(output, ReportOutput::Stdout) {
        mold_common::error::out_record("size-report", text.trim_end_matches('\n'));
        return;
    }
    if json {
        mold_common::error::tee_record(&text);
    }
    output.with_writer("--size-report", |out| std::io::Write::write_all(out, text.as_bytes()));
}

/// --link-summary: one JSON object about the link that just worked.
pub fn link_summary<E: Target>(ctx: &Context<E>, path: &std::path::Path) {
    // From here on a failure (such as an unwritable summary) must not
    // write a second, "failed" summary.
    mold_common::error::set_summary_path(None);
    let in_archives = ctx.objs.iter().filter(|f| !f.archive_name.as_os_str().is_empty()).count();
    let full = mold_common::error::detail_level() == DetailLevel::Full;
    let brief = mold_common::error::detail_level() == DetailLevel::Brief;
    let (errors, warnings) =
        (mold_common::error::error_count() as u64, mold_common::error::warning_count() as u64);
    let mut o = JsonObj::new("link-summary");
    o.str("status", "ok").str("output", &ctx.args.output.display().to_string());
    match output_file_size(ctx) {
        Some(n) => {
            o.num("output_size", n);
        }
        None if full => {
            o.null("output_size");
        }
        None => {}
    }
    if !brief {
        o.str("target", E::NAME);
    }
    if !mold_common::error::hide_times() {
        o.raw("elapsed_seconds", &format!("{:.3}", mold_common::error::elapsed_seconds()));
    }
    if !brief {
        let mut inputs = format!("{{\"objects\":{}", ctx.objs.len());
        if full || in_archives > 0 {
            write!(inputs, ",\"objects_from_archives\":{in_archives}").unwrap();
        }
        if full || !ctx.dsos.is_empty() {
            write!(inputs, ",\"shared_libraries\":{}", ctx.dsos.len()).unwrap();
        }
        inputs.push('}');
        o.raw("inputs", &inputs);
        o.num("output_sections", ctx.chunks.len() as u64);
    }
    if full || errors > 0 {
        o.num("errors", errors);
    }
    if full || warnings > 0 {
        o.num("warnings", warnings);
    }
    let mut s = o.finish();
    s.push('\n');
    mold_common::error::summary_record(&s);
    if let Err(e) = std::fs::write(path, s) {
        mold_common::error::summary_write_warning(path, &e);
    }
}

/// The compiler driver's boilerplate: the C runtime start-up files and the
/// library search paths it adds. They say nothing about the project.
fn is_driver_noise(arg: &str, prev: Option<&str>) -> bool {
    if arg.starts_with("-L") || prev == Some("-L") {
        return true;
    }
    let base = arg.rsplit('/').next().unwrap_or(arg);
    arg.ends_with(".o")
        && (base.starts_with("crt") || base == "Scrt1.o" || base == "gcrt1.o" || base == "grcrt1.o")
}

/// The arguments to show for `level`, how many there were, and how many
/// are not shown. `full` keeps the first 20 and last 5 of a long list, as
/// the first version did. Below it the driver's boilerplate goes first, then
/// a still long list keeps its head and tail with a marker in the gap.
fn shape_args(given: &[String], level: DetailLevel) -> (Vec<String>, usize, usize) {
    let total = given.len();
    if level == DetailLevel::Full {
        const HEAD: usize = 20;
        const TAIL: usize = 5;
        let omitted = total.saturating_sub(HEAD + TAIL);
        let shown = if omitted > 0 {
            given[..HEAD].iter().chain(&given[total - TAIL..]).cloned().collect()
        } else {
            given.to_vec()
        };
        return (shown, total, omitted);
    }
    let mut kept: Vec<&String> = Vec::with_capacity(total);
    for (i, a) in given.iter().enumerate() {
        if i > 0 && is_driver_noise(a, given.get(i - 1).map(String::as_str)) {
            continue;
        }
        kept.push(a);
    }
    let (head, tail) = if level == DetailLevel::Brief { (6, 6) } else { (20, 10) };
    let mut shown: Vec<String> = Vec::new();
    if kept.len() > head + tail {
        shown.extend(kept[..head].iter().map(|a| mold_common::error::stable_text(a).into_owned()));
        shown.push(format!("[{} arguments omitted]", kept.len() - head - tail));
        shown.extend(
            kept[kept.len() - tail..]
                .iter()
                .map(|a| mold_common::error::stable_text(a).into_owned()),
        );
        let shown_real = head + tail;
        (shown, total, total - shown_real)
    } else {
        shown.extend(kept.iter().map(|a| mold_common::error::stable_text(a).into_owned()));
        let n = shown.len();
        (shown, total, total - n)
    }
}

fn shell_quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./=:,+@%".contains(&b));
    if plain { arg.to_string() } else { format!("'{}'", arg.replace('\'', "'\\''")) }
}

/// --echo-command-line: the command line as given and as the linker sees
/// it once response files are expanded.
pub fn echo_command_line() {
    use mold_common::response_file::{original_argv, response_file_contents};
    let given: Vec<String> =
        original_argv().iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let files: Vec<(String, Vec<String>)> = response_file_contents()
        .into_iter()
        .map(|(p, args)| {
            (
                p.to_string_lossy().into_owned(),
                args.iter().map(|a| a.to_string_lossy().into_owned()).collect(),
            )
        })
        .collect();
    let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    if mold_common::error::json_enabled() {
        let level = mold_common::error::detail_level();
        let mut o = JsonObj::new("command-line");
        o.str("cwd", &mold_common::error::stable_text(&cwd));
        let (shown, total, omitted) = shape_args(&given, level);
        o.strs("args", &shown);
        o.num("args_total", total as u64);
        if level == DetailLevel::Full || omitted > 0 {
            o.num("args_omitted", omitted as u64);
        }
        let mut list = String::from("[");
        for (i, (path, args)) in files.iter().enumerate() {
            if i > 0 {
                list.push(',');
            }
            let (shown, total, omitted) = shape_args(args, level);
            let mut f = String::from("{\"path\":");
            json_string(&mut f, path);
            f.push_str(",\"args\":");
            mold_common::error::json_array(&mut f, &shown);
            if level != DetailLevel::Full && omitted > 0 {
                write!(f, ",\"args_total\":{total},\"args_omitted\":{omitted}").unwrap();
            }
            f.push('}');
            list.push_str(&f);
        }
        list.push(']');
        if level == DetailLevel::Full || !files.is_empty() {
            o.raw("response_files", &list);
        }
        mold_common::error::out_record("command-line", &o.finish());
    } else {
        let line: Vec<String> = given.iter().map(|a| shell_quote(a)).collect();
        mold_common::out!("command line: {}", line.join(" "));
        for (path, args) in &files {
            let line: Vec<String> = args.iter().map(|a| shell_quote(a)).collect();
            mold_common::out!("{path} expands to: {}", line.join(" "));
        }
        mold_common::out!("working directory: {cwd}");
    }
}
