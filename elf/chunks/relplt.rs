//! `.rel.plt` and `.rela.plt`, relocations for PLT entries.

use crate::arch::{Family, Target};
use crate::chunks::ChunkHeader;
use crate::context::Context;
use crate::elf::*;
use crate::symbol::Symbol;

// .rel.plt contains relocation information for .plt.
pub fn new_header<E: Target>() -> ChunkHeader<E> {
    let (name, ty) = if E::IS_RELA { (".rela.plt", SHT_RELA) } else { (".rel.plt", SHT_REL) };
    let mut hdr = ChunkHeader::<E>::new(name, ty, SHF_ALLOC as u64);
    let entsize = size_of::<ElfRel<E>>() as u64;
    hdr.shdr.sh_entsize.set(entsize);
    hdr.shdr.sh_addralign.set(E::WORD_SIZE as u64);
    hdr
}

pub fn update_shdr<E: Target>(ctx: &mut Context<E>) {
    let size = ctx.plt.symbols.len() as u64 * size_of::<ElfRel<E>>() as u64;
    ctx.relplt.shdr.sh_size.set(size);
    ctx.relplt.shdr.sh_link.set(ctx.dynsym.hdr.shndx);
    if !E::IS_SPARC {
        ctx.relplt.shdr.sh_info.set(ctx.gotplt.shndx);
    }
}

// `-z mark-plt` lets glibc 2.39 or later rewrite PLT entries into direct
// jumps once their GOT entries are resolved, if lazy binding is disabled
// and the tunable glibc.cpu.plt_rewrite asks for it.
// DT_X86_64_PLT, DT_X86_64_PLTSZ and DT_X86_64_PLTENT give the location
// and the entry size of the PLT, which glibc makes writable while it
// rewrites the entries, and the addend of each R_X86_64_JUMP_SLOT gives
// the address in the PLT entry where the new jump instruction goes.
//
// As in GNU ld, the address skips the entry's endbr64, so that the entry
// remains a valid indirect branch target if IBT is enabled. If IBT is
// disabled, glibc rewrites the entry from its beginning instead.
fn mark_plt_addend<E: Target>(ctx: &Context<E>, sym: &Symbol) -> i64 {
    if E::FAMILY != Family::X86_64 || !ctx.args.z_mark_plt {
        return 0;
    }
    // A canonical PLT entry starts with endbr64 (see x86_64.rs).
    let offset = if sym.is_canonical() { 4 } else { 0 };
    (sym.plt_addr(ctx) + offset) as i64
}

pub fn copy_buf<E: Target>(ctx: &Context<E>, buf: &mut [u8]) {
    let out = rels_from_bytes_mut::<E>(buf);
    debug_assert_eq!(out.len(), ctx.plt.symbols.len());
    for (i, &id) in ctx.plt.symbols.iter().enumerate() {
        let sym = &ctx.symbols[id];
        let rel = if E::IS_SPARC {
            // SPARC doesn't have a .got.plt because its role is merged into
            // .plt. On SPARC, .plt is writable (!) and the dynamic linker
            // directly modifies .plt's machine instructions as it resolves
            // dynamic symbols. Therefore, it doesn't need a separate section
            // to store the symbol resolution results. That is of course
            // horrible from the security point of view, though.
            let idx = sym.plt_idx(&ctx.symbols).unwrap() as u64;
            if idx < crate::arch::sparc64::SPARC_NUM_SMALL_PLT {
                ElfRel::<E>::new(
                    sym.plt_addr(ctx),
                    E::R_JUMP_SLOT,
                    sym.dynsym_idx(&ctx.symbols).unwrap_or(0),
                    0,
                )
            } else {
                // A large PLT entry resolves through a data pointer rather than
                // self-modifying code, so its relocation targets that pointer and
                // carries -(call address) as the addend, making the loader store
                // (target - call) there (see arch/sparc64.rs).
                let call = sym.plt_addr(ctx) + 4;
                let ptr = ctx.plt.hdr.shdr.sh_addr.get()
                    + crate::arch::sparc64::plt_ptr_offset(ctx.plt.symbols.len(), idx);
                ElfRel::<E>::new(
                    ptr,
                    E::R_JUMP_SLOT,
                    sym.dynsym_idx(&ctx.symbols).unwrap_or(0),
                    -(call as i64),
                )
            }
        } else {
            ElfRel::<E>::new(
                sym.gotplt_addr(ctx),
                E::R_JUMP_SLOT,
                sym.dynsym_idx(&ctx.symbols).unwrap_or(0),
                mark_plt_addend(ctx, sym),
            )
        };
        out[i] = rel;
    }
}
