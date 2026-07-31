# Attribution / Third-Party Notices

This file records the origin and licence of third-party work used by **zplint** and its
`zpc` subproject (a Rust reimplementation of the AMX Mod X Pawn compiler toolchain).

It exists to satisfy condition 3 of the zlib-style licence used by the Pawn compiler
("This notice may not be removed or altered from any source distribution") and condition 2
("Altered source versions must be plainly marked as such").

---

## 1. Pawn compiler — ITB CompuPhase

`zpc` contains code **derived from** the Pawn compiler by ITB CompuPhase (the `libpc300`
sources of the AMX Mod X distribution: `sc1.c`–`sc7.c`, `sc.h`, `sci18n.c`, `sclist.c`,
`scstate.c`, `scvars.c`, `scmemfil.c`, `libpawnc.c`, `pawncc.c`, `amx.h`, `amxdbg.h`,
`sc5-in.scp`, `sc7-in.scp`).

**zpc is an ALTERED version of the Pawn compiler.** It is a reimplementation in Rust with a
different internal architecture (AST-based rather than single-pass). It is **not** the original
software from ITB CompuPhase, must not be represented as such, and any defect in zpc is ours,
not theirs.

### Required notice (reproduced verbatim from the upstream headers)

```
/*  Pawn compiler
 *
 *  Copyright (c) ITB CompuPhase, 1997-2005
 *
 *  This software is provided "as-is", without any express or implied warranty.
 *  In no event will the authors be held liable for any damages arising from
 *  the use of this software.
 *
 *  Permission is granted to anyone to use this software for any purpose,
 *  including commercial applications, and to alter it and redistribute it
 *  freely, subject to the following restrictions:
 *
 *  1.  The origin of this software must not be misrepresented; you must not
 *      claim that you wrote the original software. If you use this software in
 *      a product, an acknowledgment in the product documentation would be
 *      appreciated but is not required.
 *  2.  Altered source versions must be plainly marked as such, and must not be
 *      misrepresented as being the original software.
 *  3.  This notice may not be removed or altered from any source distribution.
 */
```

Copyright years vary per file (1997–2005, 2000–2005, 2001–2005, 2003–2005, 2004–2005, 2005);
the licence text is identical in all of them. `sc.h` additionally carries:

```
 *  Drafted after the Small-C compiler Version 2.01, originally created
 *  by Ron Cain, july 1980, and enhanced by James E. Hendrix.
 *
 *  Copyright R. Cain, 1980
 *  Copyright J.E. Hendrix, 1982, 1983
 *  Copyright ITB CompuPhase, 1997-2005
```

Acknowledgement, as invited by condition 1: **zpc's Pawn front end and code generator are
based on the Pawn compiler by ITB CompuPhase (https://www.compuphase.com/pawn/pawn.htm).**

---

## 2. AMX Mod X — AMX Mod X Development Team

The `.amxx` plugin container, the AMX Mod X include files, and the `amxxpc` driver are the work
of the **AMX Mod X Development Team** (AMX Mod X is based on AMX Mod by Aleksander Naszko,
"OLO"). Those sources are licensed **GNU GPL v3 or later, with additional exceptions**
(see https://alliedmods.net/amxmodx-license).

**No GPL-licensed AMX Mod X code is copied, translated, or transcribed into zpc.** Where zpc
needs to interoperate with AMX Mod X (the `.amxx` container layout, the `.amx` header, plugin
loading behaviour), it is implemented **independently from documented format facts and from
observed behaviour of released binaries**, not by porting the GPL sources.

We acknowledge the AMX Mod X Development Team as the authors of the ecosystem zplint targets.

---

## 3. What we ported vs. what we reimplemented

| Area | Upstream origin | Licence | Our relationship to it |
|---|---|---|---|
| Lexer / preprocessor, parser, symbol table, tag system, codegen, assembler, peephole optimizer | `libpc300` (ITB CompuPhase) | zlib-style permissive | **Ported / derived.** Altered version — see §1. |
| Compiler diagnostic codes and message strings | `sc5-in.scp` (ITB CompuPhase) | zlib-style permissive | **Reused** for output parity. Covered by §1. |
| Peephole "sequences" table | `sc7-in.scp` (ITB CompuPhase) | zlib-style permissive | **Ported / derived.** Covered by §1. |
| `.amx` file header / AMX ABI constants | `amx.h`, `amxdbg.h` (ITB CompuPhase) | zlib-style permissive | **Ported / derived.** Covered by §1. |
| `.amxx` container (magic, version, cell size, zlib-compressed sections) | `amxxpc.cpp`, `Binary.cpp/.h`, `amxxpc.h` (AMX Mod X) | GPLv3+ | **NOT ported.** Reimplemented from format facts. No GPL code copied. |
| In-memory scratch file abstraction (`memfile`) | `memfile.c/.h` (AMX Mod X — inside `libpc300`) | GPLv3+ | **NOT ported.** Replaced by ordinary Rust buffers. See §4. |
| Symbol hash table (`sp_symhash`) | `sp_symhash.c/.h` (AlliedModders, no notice in file) | **Unclear** | **NOT ported.** Replaced by Rust `HashMap`. See §4. |
| Relocatable-executable path lookup (`prefix.c/.h`, BinReloc) | Mike Hearn, Hongli Lai | Public domain (per file header) | **Not needed.** Rust std handles paths. |

---

## 4. Notable exceptions inside `libpc300`

`libpc300` is **not uniformly zlib-licensed**. The following files in that directory are not
CompuPhase zlib code and must be treated separately:

| File | Notice found in header | Consequence |
|---|---|---|
| `memfile.c`, `memfile.h` | AMX Mod X, **GPLv3 or higher** | Treat as GPL. Do not port or transcribe. |
| `prefix.c`, `prefix.h` | BinReloc — "This source code is public domain." | Usable, but we do not need it. |
| `sp_symhash.c`, `sp_symhash.h` | **No copyright or licence header at all** | Provenance unclear (AlliedModders/SourcePawn lineage). Do not port; reimplement. |
| `getch.h`, `sclinux.h` | No licence header (short platform shims) | Do not port; trivial and unnecessary in Rust. |
| `osdefs.h` | `Copyright 1998-2005, ITB CompuPhase` — **copyright line only, no permission grant** | The zlib grant is not restated in this file. Treat the constants as facts, do not copy the file. |

---

## 5. zplint's own licence

zplint is licensed under the **Apache License, Version 2.0**. The full text is in the `LICENSE`
file at the repository root, and `license = "Apache-2.0"` is declared in `[workspace.package]`
of the root `Cargo.toml`, inherited by every publishable crate.

The Apache-2.0 grant covers zplint's own code. It does **not** override the obligations
recorded in this file: the CompuPhase zlib conditions in §1 continue to apply to the derived
code regardless of the licence we chose, and the GPL boundary in §3 continues to apply to
AMX Mod X.

Because every crate published to crates.io is an independent source distribution, each
directory under `crates/` carries its own copy of `LICENSE` and of `NOTICE` — the latter
reproduces the CompuPhase notice verbatim, satisfying condition 3 of §1. Do not remove them.

See `docs/LICENSING.md` for the engineering rules and `docs/PUBLISHING.md` for the release
procedure.
