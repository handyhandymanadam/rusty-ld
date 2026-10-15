//! Opt-in diagnostics that help a person or a program understand a failed
//! link. Nothing here runs unless an option such as --explain-undefined
//! asks for it, and none of it changes the output file.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rayon::prelude::*;

/// What we found about one undefined symbol.
#[derive(Default, Clone)]
pub struct UndefHints {
    /// Libraries that define the symbol, in search order.
    pub providers: Vec<Provider>,
    /// Names that are close to the undefined one.
    pub near_misses: Vec<NearMiss>,
}

#[derive(Clone)]
pub struct Provider {
    pub path: PathBuf,
    /// The argument that adds the library, such as `-lm`.
    pub option: String,
    /// True if the path is on the -L search list, so `-l` alone finds it.
    pub on_search_path: bool,
}

#[derive(Clone)]
pub struct NearMiss {
    pub name: String,
    /// The input file or library that defines it.
    pub origin: String,
    distance: usize,
    from_library: bool,
}

/// A symbol defined in a file given on the command line.
pub struct LocalDef {
    pub name: &'static [u8],
    pub origin: String,
}

const SYSTEM_DIRS: &[&str] =
    &["/usr/local/lib64", "/usr/local/lib", "/usr/lib64", "/lib64", "/usr/lib", "/lib"];

/// The directories to look for libraries in: the -L paths, then the
/// system's. The second element tells whether the directory came from -L.
fn search_dirs(library_paths: &[PathBuf]) -> Vec<(PathBuf, bool)> {
    let mut dirs: Vec<(PathBuf, bool)> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut add = |dir: PathBuf, from_l: bool| {
        let key = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        if key.is_dir() && seen.insert(key) {
            dirs.push((dir, from_l));
        }
    };
    for dir in library_paths {
        add(dir.clone(), true);
    }
    for dir in SYSTEM_DIRS {
        add(PathBuf::from(dir), false);
    }
    // Debian-style multiarch directories, such as /usr/lib/x86_64-linux-gnu.
    if let Ok(entries) = std::fs::read_dir("/usr/lib") {
        let mut multi: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.contains("-linux-gnu"))
            })
            .collect();
        multi.sort();
        for dir in multi {
            add(dir, false);
        }
    }
    // The dynamic loader's own list.
    for dir in ld_so_conf_dirs(Path::new("/etc/ld.so.conf"), 0) {
        add(dir, false);
    }
    dirs
}

fn ld_so_conf_dirs(conf: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(text) = std::fs::read_to_string(conf) else { return out };
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if let Some(pattern) = line.strip_prefix("include ") {
            if depth > 2 {
                continue;
            }
            let pattern = pattern.trim();
            let (dir, file_pat) = match pattern.rsplit_once('/') {
                Some((d, f)) => (PathBuf::from(if d.is_empty() { "/" } else { d }), f),
                None => (conf.parent().unwrap_or(Path::new("/")).to_path_buf(), pattern),
            };
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            let mut files: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| glob_match(file_pat, n))
                })
                .collect();
            files.sort();
            for f in files {
                out.extend(ld_so_conf_dirs(&f, depth + 1));
            }
        } else if line.starts_with('/') {
            out.push(PathBuf::from(line));
        }
    }
    out
}

/// A `*`-only glob, enough for ld.so.conf's include lines.
fn glob_match(pat: &str, s: &str) -> bool {
    match pat.split_once('*') {
        None => pat == s,
        Some((head, tail)) => {
            s.len() >= head.len() + tail.len() && s.starts_with(head) && s.ends_with(tail)
        }
    }
}

struct LibFile {
    path: PathBuf,
    /// `foo` for libfoo.so, libfoo.so.1 and libfoo.a.
    stem: String,
    /// `-lfoo` finds a file in this directory (libfoo.so or libfoo.a exists,
    /// even if libfoo.so is a linker script we cannot read).
    linkable: bool,
    on_search_path: bool,
}

/// `1`, `1.2.3`: the version part of a shared library file name.
fn is_so_suffix(rest: &str) -> bool {
    rest.split('.').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

fn list_libraries(dirs: &[(PathBuf, bool)]) -> Vec<LibFile> {
    let mut files = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for (dir, from_l) in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut names: Vec<(String, PathBuf)> = entries
            .flatten()
            .filter_map(|e| Some((e.file_name().into_string().ok()?, e.path())))
            .filter(|(n, _)| n.starts_with("lib"))
            .collect();
        names.sort();
        // The stems that `-l` can find in this directory.
        let linkable: HashSet<&str> = names
            .iter()
            .filter_map(|(n, _)| {
                let rest = n.strip_prefix("lib")?;
                rest.strip_suffix(".so").or_else(|| rest.strip_suffix(".a"))
            })
            .collect();
        for (name, path) in &names {
            let rest = &name["lib".len()..];
            let stem = if let Some(stem) = rest.strip_suffix(".a") {
                stem
            } else if let Some(i) = rest.find(".so") {
                let tail = &rest[i + 3..];
                if !(tail.is_empty() || tail.strip_prefix('.').is_some_and(is_so_suffix)) {
                    continue;
                }
                &rest[..i]
            } else {
                continue;
            };
            if stem.is_empty() {
                continue;
            }
            // libfoo.so and libfoo.so.1 are usually one file.
            let real = path.canonicalize().unwrap_or_else(|_| path.clone());
            if !seen.insert(real) {
                continue;
            }
            files.push(LibFile {
                path: path.clone(),
                stem: stem.to_string(),
                linkable: linkable.contains(stem),
                on_search_path: *from_l,
            });
        }
    }
    files
}

// ---------------------------------------------------------------------
// Reading the defined names of an ELF file without the linker's own types:
// this runs on files that may be for another word size or byte order.

struct Raw<'a> {
    data: &'a [u8],
    is64: bool,
    be: bool,
}

impl<'a> Raw<'a> {
    fn new(data: &'a [u8]) -> Option<Self> {
        if data.len() < 64 || &data[..4] != b"\x7fELF" {
            return None;
        }
        let is64 = match data[4] {
            1 => false,
            2 => true,
            _ => return None,
        };
        let be = match data[5] {
            1 => false,
            2 => true,
            _ => return None,
        };
        Some(Self { data, is64, be })
    }

    fn u16(&self, off: usize) -> Option<u64> {
        let b: [u8; 2] = self.data.get(off..off + 2)?.try_into().ok()?;
        Some(if self.be { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) } as u64)
    }

    fn u32(&self, off: usize) -> Option<u64> {
        let b: [u8; 4] = self.data.get(off..off + 4)?.try_into().ok()?;
        Some(if self.be { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) } as u64)
    }

    fn u64(&self, off: usize) -> Option<u64> {
        let b: [u8; 8] = self.data.get(off..off + 8)?.try_into().ok()?;
        Some(if self.be { u64::from_be_bytes(b) } else { u64::from_le_bytes(b) })
    }

    fn machine(&self) -> Option<u64> {
        self.u16(18)
    }

    /// Calls `f` with the name of each defined global or weak symbol in the
    /// table of section type `sh_type`.
    fn for_each_defined(&self, sh_type: u64, mut f: impl FnMut(&'a [u8])) -> Option<()> {
        let (shoff, shentsize, shnum) = if self.is64 {
            (self.u64(40)?, self.u16(58)?, self.u16(60)?)
        } else {
            (self.u32(32)?, self.u16(46)?, self.u16(48)?)
        };
        let (shoff, shentsize) = (shoff as usize, shentsize as usize);
        let sh = |i: u64| shoff + i as usize * shentsize;
        for i in 0..shnum {
            let h = sh(i);
            if self.u32(h + 4)? != sh_type {
                continue;
            }
            let (off, size, link, entsize) = if self.is64 {
                (self.u64(h + 24)?, self.u64(h + 32)?, self.u32(h + 40)?, self.u64(h + 56)?)
            } else {
                (self.u32(h + 16)?, self.u32(h + 20)?, self.u32(h + 24)?, self.u32(h + 36)?)
            };
            let entsize = if entsize == 0 { if self.is64 { 24 } else { 16 } } else { entsize };
            let strh = sh(link);
            let (stroff, strsize) = if self.is64 {
                (self.u64(strh + 24)?, self.u64(strh + 32)?)
            } else {
                (self.u32(strh + 16)?, self.u32(strh + 20)?)
            };
            let strtab = self.data.get(stroff as usize..(stroff + strsize) as usize)?;
            for s in 0..size / entsize {
                let e = (off + s * entsize) as usize;
                let (name, info, shndx) = if self.is64 {
                    (self.u32(e)?, *self.data.get(e + 4)?, self.u16(e + 6)?)
                } else {
                    (self.u32(e)?, *self.data.get(e + 12)?, self.u16(e + 14)?)
                };
                let (bind, ty) = (info >> 4, info & 0xf);
                if shndx == 0 || !(bind == 1 || bind == 2) || ty == 3 || ty == 4 {
                    continue;
                }
                let tail = strtab.get(name as usize..)?;
                let end = memchr::memchr(0, tail).unwrap_or(tail.len());
                if end > 0 {
                    f(&tail[..end]);
                }
            }
        }
        Some(())
    }
}

/// What one library file offers.
#[derive(Default)]
struct FileScan {
    exact: Vec<usize>,
    near: Vec<(String, usize, usize)>,
}

fn scan_names(data: &'static [u8], path: &Path, machine: u64, sink: &mut impl FnMut(&[u8])) {
    if data.starts_with(b"!<arch>\n") || data.starts_with(b"!<thin>\n") {
        for m in mold_common::archive_file::members(path, data) {
            if let Some(raw) = Raw::new(m.data)
                && raw.machine() == Some(machine)
            {
                raw.for_each_defined(2, |n| sink(n));
            }
        }
    } else if let Some(raw) = Raw::new(data)
        && raw.machine() == Some(machine)
    {
        raw.for_each_defined(11, |n| sink(n));
    }
}

fn max_distance(len: usize) -> usize {
    match len {
        0..=2 => 0,
        3..=8 => 1,
        9..=12 => 2,
        _ => 3,
    }
}

/// The edit distance between `a` and `b`, or None if it exceeds `limit`.
fn bounded_distance(a: &[u8], b: &[u8], limit: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > limit {
        return None;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        let mut row_min = cur[0];
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
            row_min = row_min.min(cur[j + 1]);
        }
        if row_min > limit {
            return None;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    (prev[b.len()] <= limit).then_some(prev[b.len()])
}

/// How close `candidate` is to `target`, if it is close at all. Accepts
/// the usual slips: a changed letter, a missing or extra letter, a different
/// case, and a leading underscore that came or went.
fn closeness(target: &[u8], candidate: &[u8]) -> Option<usize> {
    if target == candidate || target.len() < 2 {
        return None;
    }
    if target.eq_ignore_ascii_case(candidate) {
        return Some(1);
    }
    if candidate.strip_prefix(b"_") == Some(target) || target.strip_prefix(b"_") == Some(candidate)
    {
        return Some(1);
    }
    bounded_distance(target, candidate, max_distance(target.len()))
}

/// Looks for providers and near misses for each name in `targets`.
///
/// `local` lists what the files on the command line define.
///
/// `linked` lists the libraries already on the link (shared libraries and
/// archives): one of them is never offered again, nor another file with the
/// same `-l` name.
pub fn find_undef_hints(
    targets: &[&'static [u8]],
    library_paths: &[PathBuf],
    local: &[LocalDef],
    machine: u16,
    linked: &[PathBuf],
) -> HashMap<&'static [u8], UndefHints> {
    let machine = machine as u64;
    let mut result: HashMap<&'static [u8], UndefHints> =
        targets.iter().map(|&t| (t, UndefHints::default())).collect();
    let index: HashMap<&[u8], usize> = targets.iter().enumerate().map(|(i, &t)| (t, i)).collect();

    // Near misses among the files on the command line.
    for def in local {
        for &t in targets {
            if let Some(d) = closeness(t, def.name) {
                result.get_mut(t).unwrap().near_misses.push(NearMiss {
                    name: String::from_utf8_lossy(def.name).into_owned(),
                    origin: def.origin.clone(),
                    distance: d,
                    from_library: false,
                });
            }
        }
    }

    // Archives on the link: a member that nothing pulled in is not among the
    // files above, but a misspelling of one of its names is likely.
    let archives: Vec<(Vec<(String, usize, usize)>, String)> = linked
        .par_iter()
        .filter_map(|path| {
            let mf = mold_common::mapped_file::MappedFile::open(path)?;
            let data: &'static [u8] = mf.data();
            if !(data.starts_with(b"!<arch>\n") || data.starts_with(b"!<thin>\n")) {
                return None;
            }
            let mut near: Vec<(String, usize, usize)> = Vec::new();
            let mut seen: HashSet<(usize, Vec<u8>)> = HashSet::new();
            scan_names(data, path, machine, &mut |name| {
                let base = &name[..crate::symbol::name_len(name)];
                for (i, &t) in targets.iter().enumerate() {
                    if t.len().abs_diff(base.len()) <= 3
                        && let Some(d) = closeness(t, base)
                        && seen.insert((i, base.to_vec()))
                    {
                        near.push((String::from_utf8_lossy(base).into_owned(), i, d));
                    }
                }
            });
            Some((near, path.display().to_string()))
        })
        .collect();
    for (near, origin) in archives {
        for (name, i, d) in near {
            result.get_mut(targets[i]).unwrap().near_misses.push(NearMiss {
                name,
                origin: origin.clone(),
                distance: d,
                from_library: false,
            });
        }
    }

    // Libraries.
    let dirs = search_dirs(library_paths);
    let libs = list_libraries(&dirs);
    let scans: Vec<FileScan> = libs
        .par_iter()
        .map(|lib| {
            let mut scan = FileScan::default();
            let Some(mf) = mold_common::mapped_file::MappedFile::open(&lib.path) else {
                return scan;
            };
            let data: &'static [u8] = mf.data();
            let mut seen_exact: HashSet<usize> = HashSet::new();
            let mut seen_near: HashSet<(usize, Vec<u8>)> = HashSet::new();
            scan_names(data, &lib.path, machine, &mut |name| {
                let base = &name[..crate::symbol::name_len(name)];
                if let Some(&i) = index.get(base) {
                    if seen_exact.insert(i) {
                        scan.exact.push(i);
                    }
                    return;
                }
                for (i, &t) in targets.iter().enumerate() {
                    // A short name is within one letter of too many unrelated
                    // names in system libraries: offer such names only from
                    // the files on the command line.
                    if t.len() >= LIBRARY_NEAR_MISS_MIN
                        && t.len().abs_diff(base.len()) <= 3
                        && let Some(d) = closeness(t, base)
                        && seen_near.insert((i, base.to_vec()))
                    {
                        scan.near.push((String::from_utf8_lossy(base).into_owned(), i, d));
                    }
                }
            });
            scan
        })
        .collect();

    // What is linked already, by file and by `-l` name.
    let linked_real: HashSet<PathBuf> =
        linked.iter().map(|p| p.canonicalize().unwrap_or_else(|_| p.clone())).collect();
    let linked_stems: HashSet<String> = linked.iter().filter_map(|p| lib_stem(p)).collect();
    for (lib, scan) in libs.iter().zip(scans) {
        let real = lib.path.canonicalize().unwrap_or_else(|_| lib.path.clone());
        let already = linked_real.contains(&real) || linked_stems.contains(&lib.stem);
        for i in scan.exact {
            if already {
                continue;
            }
            let hints = result.get_mut(targets[i]).unwrap();
            let option = if lib.linkable {
                format!("-l{}", lib.stem)
            } else {
                let file = lib.path.file_name().unwrap_or_default().to_string_lossy();
                format!("-l:{file}")
            };
            if hints.providers.iter().any(|p| p.option == option) {
                continue;
            }
            hints.providers.push(Provider {
                path: lib.path.clone(),
                option,
                on_search_path: lib.on_search_path,
            });
        }
        for (name, i, d) in scan.near {
            let origin = lib.path.display().to_string();
            result.get_mut(targets[i]).unwrap().near_misses.push(NearMiss {
                name,
                origin,
                distance: d,
                from_library: true,
            });
        }
    }

    for hints in result.values_mut() {
        // Plain names such as -lm before odd ones such as -lm-2.43.
        hints.providers.sort_by_key(|p| p.option.len());
        // A versioned name (-lm-2.43) is another name for a library that
        // has a stable one (-lm): keep the stable one only.
        let stable: HashSet<String> = hints.providers.iter().map(|p| p.option.clone()).collect();
        hints.providers.retain(|p| {
            versioned_base(&p.option).is_none_or(|base| !stable.contains(&format!("-l{base}")))
        });
        // Close names found only in system libraries are mostly noise, so
        // they are offered only when nothing better is known.
        if !hints.providers.is_empty() {
            hints.near_misses.retain(|n| !n.from_library);
        }
        hints.near_misses.sort_by(|a, b| {
            (a.from_library, a.distance, a.name.len(), &a.name, &a.origin).cmp(&(
                b.from_library,
                b.distance,
                b.name.len(),
                &b.name,
                &b.origin,
            ))
        });
        hints.near_misses.dedup_by(|a, b| a.name == b.name);
        hints.near_misses.truncate(3);
    }
    result
}

/// The shortest undefined name that is matched against the names in system
/// libraries.
const LIBRARY_NEAR_MISS_MIN: usize = 6;

/// `m` for `/usr/lib64/libm.so.6` and `libm.a`.
fn lib_stem(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix("lib")?;
    let stem = if let Some(s) = rest.strip_suffix(".a") {
        s
    } else {
        &rest[..rest.find(".so")?]
    };
    (!stem.is_empty()).then(|| stem.to_string())
}

/// `m` for `-lm-2.43`: the name without its version, if it has one.
fn versioned_base(option: &str) -> Option<&str> {
    let name = option.strip_prefix("-l")?;
    let (base, version) = name.rsplit_once('-')?;
    (!base.is_empty()
        && !version.is_empty()
        && version.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && version.bytes().next().is_some_and(|b| b.is_ascii_digit()))
    .then_some(base)
}

/// The hint line for a provider, as the text output prints it after
/// `>>> hint: ` and the JSON gives as the suggestion's message.
fn provider_text(p: &Provider) -> String {
    if p.on_search_path || p.option.starts_with("-l:") {
        format!("defined in {}; add {}", p.path.display(), p.option)
    } else {
        let dir = p.path.parent().unwrap_or(Path::new("/"));
        format!(
            "defined in {}; add {} (its directory is not in the -L list, so also add -L{})",
            p.path.display(),
            p.option,
            dir.display()
        )
    }
}

/// The hint lines for one symbol, in the style of the other `>>>` lines.
pub fn format_hints(hints: &UndefHints) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for p in hints.providers.iter().take(3) {
        writeln!(out, ">>> hint: {}", provider_text(p)).unwrap();
    }
    for n in &hints.near_misses {
        writeln!(out, ">>> hint: did you mean `{}`? (defined in {})", n.name, n.origin).unwrap();
    }
    out
}

/// The same hints as structured suggestions, for the JSON output. A
/// suggestion's message is the text of the hint line.
pub fn suggestions(hints: &UndefHints) -> Vec<mold_common::error::Suggestion> {
    let mut out = Vec::new();
    for p in hints.providers.iter().take(3) {
        let option = if p.on_search_path || p.option.starts_with("-l:") {
            p.option.clone()
        } else {
            let dir = p.path.parent().unwrap_or(Path::new("/"));
            format!("-L{} {}", dir.display(), p.option)
        };
        out.push(mold_common::error::Suggestion {
            message: provider_text(p),
            option: Some(option),
            // The option says what to add.
            short: Some(format!("defined in {}", p.path.display())),
        });
    }
    for n in &hints.near_misses {
        out.push(mold_common::error::Suggestion {
            message: format!("did you mean `{}`? (defined in {})", n.name, n.origin),
            option: None,
            short: None,
        });
    }
    out
}

/// The width in bytes of an absolute data relocation, from its name, or
/// None for any other relocation.
pub fn abs_reloc_width(name: &str) -> Option<usize> {
    const NOT: &[&str] = &["PC", "REL", "GOT", "PLT", "TLS", "TPOFF", "DTPOFF", "DTPMOD", "GP"];
    if NOT.iter().any(|n| name.contains(n)) {
        return None;
    }
    let name = name.strip_suffix('S').unwrap_or(name);
    if name.ends_with("32") {
        Some(4)
    } else if name.ends_with("64") {
        Some(8)
    } else {
        None
    }
}

// ---------------------------------------------------------------------
// --agent-help: what agent mode offers, as one JSON object. One table
// each, so the text and the list of codes cannot drift apart from this.

/// (option, argument, in the --agent-mode umbrella, what it does)
const OPTIONS: &[(&str, &str, bool, &str)] = &[
    ("--agent-mode", "", false, "turn agent mode on (same as RUSTY_AGENT=1); switches on the options marked in_agent_mode"),
    ("--no-agent-mode", "", false, "turn agent mode off again, also when RUSTY_AGENT=1; the last of the two options on the command line wins, wherever it stands"),
    ("--agent-detail=LEVEL", "LEVEL", false, "how much each record says: brief, normal (default in agent mode) or full; also RUSTY_AGENT_DETAIL=LEVEL, the option wins"),
    ("--agent-help", "", false, "print this description as one JSON object on standard output and exit (works without agent mode)"),
    ("--diagnostics-format=json", "", true, "every error, warning and fatal error as one `diagnostic` record on standard error; =text turns it off"),
    ("--explain-undefined", "", true, "for each undefined symbol: the places that refer to it (with source lines when the object has line tables), libraries that define it, close spellings"),
    ("--explain-duplicates", "", true, "for each duplicate symbol: the source file and line of each definition (needs -g)"),
    ("--stable-diagnostics", "", true, "hold diagnostics and print them sorted at the end; temporary file names written as XXXXXX; no elapsed times"),
    ("--no-stable-diagnostics", "", false, "turn the stable mode off, also inside agent mode"),
    ("--echo-command-line", "", false, "a `command-line` record on standard output (the driver's start-up files and -L options are left out below full)"),
    ("--why-kept=NAME", "NAME", false, "a `why` record per section matching NAME (symbol or section name, * matches any text): why --gc-sections kept it, or --icf folded it"),
    ("--why-removed=NAME", "NAME", false, "the same question, asked about a section you expect to be gone; needs --gc-sections or --icf"),
    ("--link-summary=FILE", "FILE", false, "a `link-summary` record in FILE; in agent mode also on standard error and the copy channels"),
    ("--size-report[=FILE]", "FILE", false, "a `size-report` record: bytes per output section and input file (standard output lists the largest; a FILE.json lists all)"),
];

const RECORDS: &[(&str, &str)] = &[
    ("diagnostic", "schema type code severity message [files symbols notes suggestions locations rendered: full] locations{object file line function} references_omitted notes suggestions{message option} (normal); brief: location fix more_locations more_fixes note_count"),
    ("command-line", "schema type cwd args args_total args_omitted response_files{path args}; args has a marker string '[N arguments omitted]' in the gap"),
    ("size-report", "schema type output output_size sections{name size alloc inputs} sections_omitted files{file size sections} files_omitted omitted_size more"),
    ("why", "schema type query section by decision(kept removed folded not-folded unknown) chain chain_omitted; brief: reason chain_length"),
    ("link-summary", "schema type status(ok failed) output output_size target elapsed_seconds(not in stable mode) inputs{objects objects_from_archives shared_libraries} output_sections errors warnings"),
    ("records-omitted", "schema type records_omitted by_type more: the size budget kept records off standard error and output (12 KB soft, 20 KB hard); the copy channels have them all"),
    ("help", "this object (--agent-help)"),
];

const CODES: &[(&str, &str, &str)] = &[
    ("L0001", "error|warning", "undefined symbol"),
    ("L0002", "error", "duplicate symbol"),
    ("L0003", "error", "library not found"),
    ("L0004", "error", "cannot open a file"),
    ("L0005", "error", "relocation out of range"),
    ("L0006", "error|warning", "unknown command-line option"),
    ("L0007", "error", "multiple common symbols"),
    ("L0008", "error", "relocation against a discarded COMDAT section"),
    ("L0009", "error", "undefined symbol in a shared library"),
    ("L0010", "error", "relocation against a symbol in a read-only section"),
    ("L0011", "warning", "DT_TEXTREL"),
    ("L0012", "warning", "RUSTY_AGENT_OUT cannot be opened"),
    ("L0013", "warning", "RUSTY_AGENT_FD cannot be used"),
    ("L0014", "warning", "--link-summary file cannot be written (agent mode only; the link has otherwise finished)"),
    ("L0100", "error", "any other error"),
    ("L0200", "warning", "any other warning"),
    ("L0300", "fatal", "any other fatal error"),
];

/// The `--agent-help` object, one line of JSON.
pub fn agent_help() -> String {
    use mold_common::error::{JsonObj, json_string};
    let list = |items: Vec<String>| format!("[{}]", items.join(","));
    let obj = |fields: &[(&str, &str)]| {
        let mut s = String::from("{");
        for (i, (k, v)) in fields.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push('"');
            s.push_str(k);
            s.push_str("\":");
            json_string(&mut s, v);
        }
        s.push('}');
        s
    };
    let mut o = JsonObj::new("help");
    o.str("tool", "rusty-ld").str("version", &crate::cmdline::VERSION);
    o.raw(
        "switch",
        r#"{"env":"RUSTY_AGENT=1","flag":"--agent-mode","other_values_change_nothing":true}"#,
    );
    o.strs(
        "umbrella_features",
        &["diagnostics-format=json", "explain-undefined", "explain-duplicates", "stable-diagnostics"]
            .map(String::from),
    );
    o.raw(
        "records_file",
        &obj(&[
            ("env", "RUSTY_AGENT_OUT=<path>, RUSTY_AGENT_FD=<n>"),
            ("what", "in agent mode every JSON record is also appended to the file / written to the open descriptor, one per line; they get every record, shaped by the detail level but without the size budget; a bad value is warning L0012 / L0013"),
        ]),
    );
    o.raw(
        "detail",
        &obj(&[
            ("env", "RUSTY_AGENT_DETAIL=brief|normal|full"),
            ("flag", "--agent-detail=LEVEL"),
            ("default", "normal in agent mode; without agent mode records are as they always were (full)"),
            ("brief", "code, message, one place, one fix, counts"),
            ("normal", "what is needed to act: no rendered text, files or symbols, no empty or zero fields (an absent field means empty, zero or null), nothing said twice, lists capped with *_omitted counts, temporary names and times left out in stable mode"),
            ("full", "every field as the first version wrote them (plus the additions listed in docs/mold.md)"),
        ]),
    );
    o.str(
        "records_on",
        "diagnostics and the link summary on standard error, answers (command-line, why, size-report) on standard output, one JSON object per line, schema and type first",
    );
    o.raw(
        "options",
        &list(
            OPTIONS
                .iter()
                .map(|(n, a, u, d)| {
                    let mut s = String::from("{\"option\":");
                    json_string(&mut s, n);
                    s.push_str(",\"argument\":");
                    json_string(&mut s, a);
                    s.push_str(&format!(",\"in_agent_mode\":{u},\"description\":"));
                    json_string(&mut s, d);
                    s.push('}');
                    s
                })
                .collect(),
        ),
    );
    o.raw(
        "record_types",
        &list(RECORDS.iter().map(|(t, f)| obj(&[("type", t), ("fields", f)])).collect()),
    );
    o.raw(
        "codes",
        &list(
            CODES
                .iter()
                .map(|(c, sv, m)| obj(&[("code", c), ("severity", sv), ("meaning", m)]))
                .collect(),
        ),
    );
    o.str("exit_code", "as without agent mode: 0 if the link worked, 1 if it failed");
    o.str("docs", "docs/mold.md, section Agent mode");
    o.finish()
}
