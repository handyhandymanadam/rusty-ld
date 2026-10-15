//! Finds the source file and line where an object file defines a symbol,
//! from the DWARF in the object file. Used by --explain-duplicates.
//!
//! Object files carry relocations in their debug sections (string
//! offsets, for one), so we apply them to a copy of each section first.
//! Sections of the object file itself are read through a small reader
//! of our own that works for any word size and byte order.

use gimli::{AttributeValue, EndianSlice, RunTimeEndian};

/// Where a symbol is defined.
#[derive(Clone, Debug)]
pub struct SourceLoc {
    pub file: String,
    pub line: u64,
}

struct Section {
    name: String,
    sh_type: u32,
    flags: u64,
    offset: usize,
    size: usize,
    link: u32,
    info: u32,
}

struct Elf<'a> {
    data: &'a [u8],
    is64: bool,
    be: bool,
    sections: Vec<Section>,
}

impl<'a> Elf<'a> {
    fn u(&self, off: usize, n: usize) -> Option<u64> {
        let b = self.data.get(off..off + n)?;
        let mut v = 0u64;
        for i in 0..n {
            let byte = if self.be { b[i] } else { b[n - 1 - i] };
            v = (v << 8) | byte as u64;
        }
        Some(v)
    }

    fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() < 64 || &data[..4] != b"\x7fELF" {
            return None;
        }
        let is64 = data[4] == 2;
        let be = data[5] == 2;
        let mut elf = Elf { data, is64, be, sections: Vec::new() };
        let (shoff, shentsize, shnum, shstrndx) = if is64 {
            (elf.u(40, 8)?, elf.u(58, 2)?, elf.u(60, 2)?, elf.u(62, 2)?)
        } else {
            (elf.u(32, 4)?, elf.u(46, 2)?, elf.u(48, 2)?, elf.u(50, 2)?)
        };
        let mut raw = Vec::new();
        for i in 0..shnum {
            let h = (shoff + i * shentsize) as usize;
            let (name, ty, flags, off, size, link, info) = if is64 {
                (
                    elf.u(h, 4)?,
                    elf.u(h + 4, 4)?,
                    elf.u(h + 8, 8)?,
                    elf.u(h + 24, 8)?,
                    elf.u(h + 32, 8)?,
                    elf.u(h + 40, 4)?,
                    elf.u(h + 44, 4)?,
                )
            } else {
                (
                    elf.u(h, 4)?,
                    elf.u(h + 4, 4)?,
                    elf.u(h + 8, 4)?,
                    elf.u(h + 16, 4)?,
                    elf.u(h + 20, 4)?,
                    elf.u(h + 24, 4)?,
                    elf.u(h + 28, 4)?,
                )
            };
            raw.push((
                name as usize,
                ty as u32,
                flags,
                off as usize,
                size as usize,
                link as u32,
                info as u32,
            ));
        }
        let strtab = {
            let s = raw.get(shstrndx as usize)?;
            data.get(s.3..s.3.checked_add(s.4)?)?
        };
        for (name, ty, flags, offset, size, link, info) in raw {
            let tail = strtab.get(name..)?;
            let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
            elf.sections.push(Section {
                name: String::from_utf8_lossy(&tail[..end]).into_owned(),
                sh_type: ty,
                flags,
                offset,
                size,
                link,
                info,
            });
        }
        Some(elf)
    }

    fn contents(&self, s: &Section) -> Option<&'a [u8]> {
        if s.sh_type == 8 {
            return Some(&[]);
        }
        self.data.get(s.offset..s.offset.checked_add(s.size)?)
    }

    /// The contents of a section, decompressed.
    fn plain(&self, s: &Section) -> Option<Vec<u8>> {
        let raw = self.contents(s)?;
        if s.flags & 0x800 == 0 {
            return Some(raw.to_vec());
        }
        let (hdr, ty, size) = if self.is64 {
            (24, self.u(s.offset, 4)?, self.u(s.offset + 8, 8)?)
        } else {
            (12, self.u(s.offset, 4)?, self.u(s.offset + 4, 4)?)
        };
        let body = raw.get(hdr..)?;
        let mut out = vec![0u8; size as usize];
        match ty {
            1 => mold_common::compress::zlib_decompress(body, &mut out).ok()?,
            2 => mold_common::compress::zstd_decompress(body, &mut out).ok()?,
            _ => return None,
        }
        Some(out)
    }

    /// Symbol values of the symbol table section `symtab`.
    fn symbol_value(&self, symtab: &Section, idx: u64) -> u64 {
        let entsize = if self.is64 { 24 } else { 16 };
        let e = symtab.offset + idx as usize * entsize;
        if self.is64 { self.u(e + 8, 8).unwrap_or(0) } else { self.u(e + 4, 4).unwrap_or(0) }
    }
}

impl Elf<'_> {
    /// The section index of symbol `idx` of the symbol table `symtab`.
    fn symbol_shndx(&self, symtab: &Section, idx: u64) -> u64 {
        let entsize = if self.is64 { 24 } else { 16 };
        let e = symtab.offset + idx as usize * entsize;
        if self.is64 { self.u(e + 6, 2).unwrap_or(0) } else { self.u(e + 14, 2).unwrap_or(0) }
    }
}

/// The debug sections of `elf` with the relocations applied, so offsets
/// that the linker would fill in are there to read. With `tag_code`, an
/// 8-byte relocation in `.debug_line` against a symbol of an executable
/// section also carries that section's index in bits 40 and up, so that
/// addresses in different sections stay apart.
fn debug_buffers(
    elf: &Elf<'_>,
    is_rela: bool,
    width_of: &dyn Fn(u32) -> Option<usize>,
    tag_code: bool,
) -> Option<Vec<(String, Vec<u8>)>> {
    let be = elf.be;
    let mut bufs: Vec<(String, Vec<u8>)> = Vec::new();
    for (i, s) in elf.sections.iter().enumerate() {
        if !s.name.starts_with(".debug_") || s.sh_type == 4 || s.sh_type == 9 {
            continue;
        }
        let mut buf = elf.plain(s)?;
        if is_rela {
            for r in &elf.sections {
                if r.sh_type != 4 || r.info as usize != i {
                    continue;
                }
                let symtab = elf.sections.get(r.link as usize)?;
                let relsz = if elf.is64 { 24 } else { 12 };
                let rb = elf.contents(r)?;
                for k in 0..rb.len() / relsz {
                    let e = r.offset + k * relsz;
                    let (off, info, addend) = if elf.is64 {
                        (elf.u(e, 8)?, elf.u(e + 8, 8)?, elf.u(e + 16, 8)? as i64)
                    } else {
                        (elf.u(e, 4)?, elf.u(e + 4, 4)?, elf.u(e + 8, 4)? as u32 as i32 as i64)
                    };
                    let (sym, ty) = if elf.is64 {
                        (info >> 32, info as u32)
                    } else {
                        (info >> 8, info as u8 as u32)
                    };
                    let Some(w) = width_of(ty) else { continue };
                    let mut value = elf.symbol_value(symtab, sym).wrapping_add(addend as u64);
                    if tag_code && w == 8 && s.name == ".debug_line" {
                        let shndx = elf.symbol_shndx(symtab, sym);
                        if elf.sections.get(shndx as usize).is_some_and(|t| t.flags & 4 != 0) {
                            value |= shndx << 40;
                        }
                    }
                    let off = off as usize;
                    if off + w > buf.len() {
                        continue;
                    }
                    for b in 0..w {
                        let shift = if be { (w - 1 - b) * 8 } else { b * 8 };
                        buf[off + b] = (value >> shift) as u8;
                    }
                }
            }
        }
        bufs.push((s.name.clone(), buf));
    }
    Some(bufs)
}

fn load_dwarf<'a>(
    bufs: &'a [(String, Vec<u8>)],
    be: bool,
) -> Option<gimli::Dwarf<EndianSlice<'a, RunTimeEndian>>> {
    let endian = if be { RunTimeEndian::Big } else { RunTimeEndian::Little };
    gimli::Dwarf::load(|id| -> Result<EndianSlice<'_, RunTimeEndian>, gimli::Error> {
        let data: &[u8] =
            bufs.iter().find(|(n, _)| n == id.name()).map(|(_, b)| b.as_slice()).unwrap_or(&[]);
        Ok(EndianSlice::new(data, endian))
    })
    .ok()
}

/// `width_of` gives the number of bytes an absolute relocation of the given
/// type writes, or None for a relocation we do not apply.
pub fn find_definition(
    data: &[u8],
    name: &[u8],
    is_rela: bool,
    width_of: &dyn Fn(u32) -> Option<usize>,
) -> Option<SourceLoc> {
    let elf = Elf::parse(data)?;
    let bufs = debug_buffers(&elf, is_rela, width_of, false)?;
    let dwarf = load_dwarf(&bufs, elf.be)?;
    find_in_dwarf(&dwarf, name)
}

/// The source file and line of each place `(section index, offset)` in
/// `places`, from the line tables of a 64-bit RELA object file. A place
/// without a line stays None; so does everything for a file of another
/// kind (32-bit, REL), which cannot be read this way.
pub fn find_lines(
    data: &[u8],
    places: &[(u32, u64)],
    is_rela: bool,
    width_of: &dyn Fn(u32) -> Option<usize>,
) -> Vec<Option<SourceLoc>> {
    let mut result: Vec<Option<SourceLoc>> = vec![None; places.len()];
    let Some(elf) = Elf::parse(data) else { return result };
    if !is_rela || !elf.is64 {
        return result;
    }
    let Some(bufs) = debug_buffers(&elf, is_rela, width_of, true) else { return result };
    let Some(dwarf) = load_dwarf(&bufs, elf.be) else { return result };
    let targets: Vec<u64> =
        places.iter().map(|&(shndx, off)| ((shndx as u64) << 40).wrapping_add(off)).collect();
    let mut units = dwarf.units();
    while let Ok(Some(header)) = units.next() {
        let Ok(unit) = dwarf.unit(header) else { continue };
        let Some(program) = unit.line_program.clone() else { continue };
        let mut rows = program.rows();
        // The previous row of the current sequence: its address, file and line.
        let mut prev: Option<(u64, u64, u64)> = None;
        while let Ok(Some((_, row))) = rows.next_row() {
            let addr = row.address();
            if let Some((paddr, pfile, pline)) = prev {
                for (i, &t) in targets.iter().enumerate() {
                    if result[i].is_none() && paddr <= t && t < addr {
                        if pline == 0 {
                            continue;
                        }
                        if let Some(file) = file_name(&dwarf, &unit, pfile) {
                            result[i] = Some(SourceLoc { file, line: pline });
                        }
                    }
                }
            }
            prev = if row.end_sequence() {
                None
            } else {
                Some((addr, row.file_index(), row.line().map_or(0, |l| l.get())))
            };
        }
    }
    result
}

type R<'a> = EndianSlice<'a, RunTimeEndian>;

fn find_in_dwarf(dwarf: &gimli::Dwarf<R<'_>>, name: &[u8]) -> Option<SourceLoc> {
    let mut units = dwarf.units();
    while let Ok(Some(header)) = units.next() {
        let Ok(unit) = dwarf.unit(header) else { continue };
        let mut cursor = unit.entries();
        while let Ok(Some(entry)) = cursor.next_dfs() {
            let tag = entry.tag();
            if tag != gimli::DW_TAG_subprogram && tag != gimli::DW_TAG_variable {
                continue;
            }
            if let Some(AttributeValue::Flag(true)) = entry.attr_value(gimli::DW_AT_declaration) {
                continue;
            }
            if !matches_name(dwarf, &unit, entry, name) {
                continue;
            }
            // A definition has code or storage. Skip the empty shells.
            let has_body = [
                gimli::DW_AT_low_pc,
                gimli::DW_AT_ranges,
                gimli::DW_AT_location,
                gimli::DW_AT_const_value,
            ]
            .iter()
            .any(|&a| entry.attr_value(a).is_some());
            if !has_body {
                continue;
            }
            if let Some(loc) = decl_of(dwarf, &unit, entry) {
                return Some(loc);
            }
        }
    }
    None
}

fn string_of(
    dwarf: &gimli::Dwarf<R<'_>>,
    unit: &gimli::Unit<R<'_>>,
    v: AttributeValue<R<'_>>,
) -> Option<Vec<u8>> {
    Some(dwarf.attr_string(unit, v).ok()?.slice().to_vec())
}

fn matches_name<'a>(
    dwarf: &gimli::Dwarf<R<'a>>,
    unit: &gimli::Unit<R<'a>>,
    entry: &gimli::DebuggingInformationEntry<R<'a>>,
    name: &[u8],
) -> bool {
    let mut e = entry.clone();
    for _ in 0..4 {
        for at in [gimli::DW_AT_linkage_name, gimli::DW_AT_MIPS_linkage_name, gimli::DW_AT_name] {
            if let Some(v) = e.attr_value(at)
                && string_of(dwarf, unit, v).is_some_and(|s| s == name)
            {
                return true;
            }
        }
        match follow(unit, &e) {
            Some(next) => e = next,
            None => return false,
        }
    }
    false
}

fn follow<'a>(
    unit: &gimli::Unit<R<'a>>,
    entry: &gimli::DebuggingInformationEntry<R<'a>>,
) -> Option<gimli::DebuggingInformationEntry<R<'a>>> {
    for at in [gimli::DW_AT_specification, gimli::DW_AT_abstract_origin] {
        if let Some(AttributeValue::UnitRef(off)) = entry.attr_value(at) {
            return unit.entry(off).ok();
        }
    }
    None
}

fn decl_of<'a>(
    dwarf: &gimli::Dwarf<R<'a>>,
    unit: &gimli::Unit<R<'a>>,
    entry: &gimli::DebuggingInformationEntry<R<'a>>,
) -> Option<SourceLoc> {
    // A C++ definition keeps its line, and often leaves the file to the
    // declaration it specifies.
    let mut e = entry.clone();
    let (mut line, mut file) = (None, None);
    for _ in 0..4 {
        if line.is_none() {
            line = e.attr_value(gimli::DW_AT_decl_line).and_then(|v| v.udata_value());
        }
        if file.is_none() {
            file = e.attr_value(gimli::DW_AT_decl_file).and_then(|v| match v {
                AttributeValue::FileIndex(i) => Some(i),
                v => v.udata_value(),
            });
        }
        match follow(unit, &e) {
            Some(next) => e = next,
            None => break,
        }
    }
    Some(SourceLoc { file: file_name(dwarf, unit, file?)?, line: line? })
}

fn file_name(dwarf: &gimli::Dwarf<R<'_>>, unit: &gimli::Unit<R<'_>>, index: u64) -> Option<String> {
    let header = unit.line_program.as_ref()?.header();
    let file = header.file(index)?;
    let name = String::from_utf8_lossy(&string_of(dwarf, unit, file.path_name())?).into_owned();
    if name.starts_with('/') {
        return Some(name);
    }
    let dir = file
        .directory(header)
        .and_then(|d| string_of(dwarf, unit, d))
        .map(|d| String::from_utf8_lossy(&d).into_owned());
    let comp_dir = unit.comp_dir.map(|d| String::from_utf8_lossy(d.slice()).into_owned());
    let mut path = String::new();
    match (&dir, &comp_dir) {
        (Some(d), _) if d.starts_with('/') => path.push_str(d),
        (Some(d), Some(c)) => {
            path.push_str(c);
            path.push('/');
            path.push_str(d);
        }
        (Some(d), None) => path.push_str(d),
        (None, Some(c)) => path.push_str(c),
        (None, None) => {}
    }
    if !path.is_empty() && !path.ends_with('/') {
        path.push('/');
    }
    path.push_str(&name);
    Some(path)
}
