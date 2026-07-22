# zplint Knowledge Base

Curated from web research (July 2026): AMXX compiler sources, AlliedModders wiki/forums,
amxmodx.org API docs, CompuPhase Pawn docs, ZP 5.0 official sources. 118 raw findings from
8 research angles, deduped and organized by category. Each entry: problem → consequence →
detection notes → sources. Entries marked `[rule: <id>]` are implemented as zplint detectors.

Meta-constraint: amxxpc (libpc300) defines warnings **200–234 only** (verified in
`compiler/libpc300/sc5-in.scp`; the array ends at 234 "recursive function"). Codes 235–240
exist only in the SA-MP fork. zplint must never reference codes >234 as amxxpc warnings.

---

## 1. Compile Errors (fatal, amxxpc)

### 1.1 Unbalanced braces (errors 054 / 030 / 010 / 004 / 055) `[rule: unbalanced_braces]`
The most common compile-failure family. Extra `}` → "error 054: unmatched closing brace";
missing `}` → "error 030: compound statement not closed at the end of file". A missing brace
in one function cascades into error 010 ("invalid function or declaration") and error 004 for
every function BELOW it — the true error is usually in the function *above* the reported line.
Detection: strip comments + string/char literals (Pawn escape is `^`), count `{`/`}`; depth <0
at any point = 054 candidate at that line; depth >0 at EOF = 030, report last unclosed `{`.
Sources: https://github.com/pawn-lang/compiler/blob/master/source/compiler/sc5.c ·
https://sampwiki.blast.hk/wiki/Errors_List · https://sampforum.blast.hk/showthread.php?tid=376412

### 1.2 String compared with `==`/`!=` (error 033) `[rule: string_literal_compare]`
Pawn cannot compare arrays with relational operators; `if (name == "admin")` gives
"error 033: array must be indexed" or compares addresses (always false). Use `equal()`/`equali()`.
Detection: `[=!]=\s*"` in conditions; exclude single-quoted char comparisons (`s[0] == 'x'` is valid).
Sources: https://forums.alliedmods.net/showthread.php?t=90368 · https://wiki.alliedmods.net/Pawn_tutorial

### 1.3 Direct string assignment (errors 047 / 006) `[rule: string_assign]`
`msg = "Hello World!"` outside a declaration only compiles when array sizes match exactly;
longer literal → "error 047: array sizes do not match". Documented safe form:
`copy(dest, charsmax(dest), "text")`. Detection: `^ident = "lit"` on non-declaration lines.
Sources: https://wiki.alliedmods.net/Pawn_tutorial · https://sampwiki.blast.hk/wiki/Errors_List

### 1.4 `else (cond)` instead of `else if (cond)` (errors 029/010/076) `[rule: else_paren]`
`else (item == 1) { }` parses the parenthesized expression as a statement and derails the
parser. Near-zero false-positive regex: `\belse\s*\(`.
Sources: https://sampforum.blast.hk/showthread.php?tid=376412

### 1.5 Semicolon after header / rogue semicolon (errors 055 / 036) `[rule: empty_statement]`
`public foo();` followed by `{` → error 055. `if (cond);` → empty statement; the block below
always runs — a logic bug even when it compiles.
Sources: https://sampwiki.blast.hk/wiki/Errors_List

### 1.6 Unterminated string literal (error 037) `[rule: unterminated_string]`
Missing closing quote; followed by a cascade of bogus errors on the same line. Detection:
odd count of unescaped `"` per line (handle `^"` and `\"` escapes).
Sources: https://github.com/pawn-lang/compiler/blob/master/source/compiler/sc5.c

### 1.7 Input line too long (error 075) `[rule: line_too_long]`
Line length limit applies AFTER macro substitution; backslash continuation does NOT help
(continued lines are joined into one logical line). Old amxxpc (1.8.2) is stricter; AMXX 1.9
raised the limit. Typical trigger: giant format()/SQL strings.
Sources: https://sampforum.blast.hk/showthread.php?tid=411657 ·
https://wiki.alliedmods.net/AMX_Mod_X_1.9_API_Changes

### 1.8 Missing #include for native prefix (error 017) 
"error 017: undefined symbol" — most-asked compile error. Map: `cs_*` → `<cstrike>`,
`pev/set_pev/engfunc/dllfunc` → `<fakemeta>`, `RegisterHam/ExecuteHam/Ham_*` → `<hamsandwich>`,
`set_user_health/set_user_gravity` → `<fun>`, `entity_get_*/find_ent_by_class` → `<engine>`,
`SQL_*` → `<sqlx>`, `zp_*` → zombieplague/zp50 includes. Also caused by UTF-8 BOM before
the first `#include`. (Not yet a detector: needs an accurate prefix→include table to stay FP-safe.)
Sources: https://forums.alliedmods.net/archive/index.php/t-292177.html

### 1.9 Unbalanced preprocessor #if/#endif (errors 026 / 060 / 061) `[rule: unbalanced_preprocessor]`
`#endif`/`#else` with no open `#if` → error 026; missing `#endif` corrupts the rest of the
parse; two `#else` in one frame → 060; `#elseif` after `#else` → 061. Frequent in ZP plugins
with `#if defined ZP50_SUPPORT` blocks copied incompletely. Detection: directive stack scan.
Sources: https://github.com/pawn-lang/compiler/blob/master/source/compiler/sc5.c

### 1.10 Duplicate global declaration (error 021)
Two `new g_x` globals after merging code. Detection: collect depth-0 declarations, flag
intra-file duplicates. (Deferred: needs enum/const tracking to avoid FPs.)
Sources: https://forums.alliedmods.net/archive/index.php/t-335882.html

### 1.11 Stacked `case` labels — Pawn has NO fallthrough `[rule: stacked_case]`
`case A:` immediately followed by `case B:` is a compile error in Pawn ("error 014/040");
C-style shared bodies need list syntax `case A, B:`. Also `break` at end of case is noise.
Sources: https://www.compuphase.com/pawn/pawnfeatures.htm

### 1.12 Multi-dimensional array initializer mismatch (error 052)
`new a[3][2] = {{...},{...}}` — rows given ≠ declared. Common in hand-edited ZP data tables.
(Deferred: initializer spans multiple lines; needs statement joining.)
Sources: https://github.com/pawn-lang/compiler/blob/master/source/compiler/sc5.c

### 1.13 Array declared with variable size (errors 008/009)
`new list[count]` where count is runtime. Detection needs #define/const/enum table. (Deferred.)
Sources: https://wiki.alliedmods.net/Pawn_tutorial

### 1.14 Known forward with wrong arity (error 025) `[rule: forward_arity]`
`public plugin_init(id)` etc. — heading differs from prototype in the include. Conservative
table: plugin_init/plugin_cfg/plugin_precache/plugin_end/plugin_natives take 0 args;
client_putinserver/client_command/client_infochanged take exactly 1.
Sources: https://sampwiki.blast.hk/wiki/Errors_List · https://forums.alliedmods.net/archive/index.php/t-630.html

### 1.15 Literal out-of-bounds array index `[rule: array_index_oob]`
From the AMXX scripting primer's own worked example: `new Players[32]` has valid slots
0..31, so `Players[32] = 15` (off-by-one, the classic beginner mistake) and `Players[-1] = 6`
are both invalid — amxxpc rejects a constant out-of-range index at compile time
(AMX_ERR_BOUNDS class). zplint checks this ahead of compilation using the whole-file
`name -> declared size` map already built for `string_assign`. Scoped narrowly to WRITES
(`name[N] =`, not reads/comparisons) on non-declaration lines, because a naive access-style
regex misreads later arrays in a multi-var `new a[4], b[32]` statement as an access to
themselves (`b[32]` has no `new` immediately in front of it) — false-positived hundreds of
times on the real-world corpus before this fix; 0 hits after restricting to the write form.
2-D arrays are only bound-checked on their first (outer) index; the second dimension isn't
tracked (deferred, see 1.12).
**Second real-corpus fix:** the shared `array_sizes` map is whole-file and first-declaration-
wins, which is an acceptable approximation for `string_assign` (only makes it more permissive)
but not for OOB bound-checking, which needs the *exact* size. `sh_uchiha.sma` in the 542-file
corpus declares a local `parm[1]` in one function and an unrelated local `parm[2]` in another;
`array_index_oob` used the cached size-1 for both, flagging `parm[1] = 40` (valid for the
size-2 declaration) as out of bounds. Fixed with a second, stricter map
(`array_sizes_unambiguous`) that drops any name seen with two different declared sizes anywhere
in the file, rather than keeping whichever was found first. 0 hits on both corpora after the fix.
Sources: https://www.amxmodx.org/doc/source/scripting/primer.htm (section 2, "Arrays") ·
real-corpus regression case (`sh_uchiha.sma`)

### 1.16 Array compared by reference with ==/!= `[rule: array_compare_by_ref]`
Also from the primer: `if (arrayOne == arrayTwo)` does not compile in Pawn — arrays are
non-scalar, so `==`/`!=` on two bare array identifiers compares references, not contents (the
primer's own example: `if ((arrayOne[0] == arrayTwo[0]) && ...)` is the correct element-wise
form; `equal()`/`equali()` is the idiomatic form for strings). zplint flags `arr1 == arr2` /
`arr1 != arr2` only when BOTH identifiers are in the declared-array-size map and neither is
immediately followed by `[` (which would mean a valid per-element compare). 0 hits on both
validation corpora (542-file real-world collection and the official amxmodx bundled plugins) —
expected, since code that fails to compile does not usually ship, but the check is cheap and a
real safety net for a plugin mid-edit.
Sources: https://www.amxmodx.org/doc/source/scripting/primer.htm (sections 2 and 8, "Arrays" / "Two Dimensional Arrays")

---

## 2. Compiler Warnings That Are Real Bugs

### 2.1 Assignment in condition (warning 211) `[rule: assignment_in_condition]`
`if (g_mode = 1)` assigns then tests. Documented intentional idiom is double parens
`if ((ent = find_ent(...)))` — do not flag it. 
Sources: https://raw.githubusercontent.com/alliedmodders/amxmodx/master/compiler/libpc300/sc5-in.scp ·
https://sampwiki.blast.hk/wiki/Errors_List

### 2.2 Comparison as statement (warning 215) `[rule: comparison_as_statement]`
`g_mode == MODE_SURVIVOR;` — the intended assignment never happens.
Sources: https://raw.githubusercontent.com/alliedmodders/amxmodx/master/compiler/libpc300/sc5-in.scp

### 2.3 Self-assignment (warning 226) `[rule: self_assignment]`
`g_class[id] = g_class[id];` — almost always a typo for a different source variable.
Sources: amxxpc sc5-in.scp (message table)

### 2.4 int literal in Float slot (warning 213) `[rules: set_task_int_interval, pev_float_int]`
Pawn does NOT convert: the integer bit pattern is reinterpreted as IEEE float.
`set_task(10, ...)` schedules ~1.4e-44 s (runs every frame). `set_pev(id, pev_health, 100)`
sets health to ~1.4e-43 (instant death). Float pev fields: health, gravity, maxspeed, speed,
dmg, takedamage, animtime, framerate, scale, renderamt, frame, fuser1-4.
Sources: https://forums.alliedmods.net/archive/index.php/t-263844.html ·
https://github.com/alliedmodders/amxmodx/blob/master/modules/fakemeta/pev.h ·
https://wiki.alliedmods.net/Tags_(Scripting)

### 2.5 Float literal in int slot (reverse 213) `[rule: int_native_float]`
`set_user_health(id, 100.0)` → 1120403456 hp (bit pattern as int). Int-param natives:
set_user_health, set_user_armor, set_user_frags, cs_set_user_money, zp_ammopacks_set.
Sources: http://www.amxmodx.org/doc/source/scripting/primer.htm

### 2.5b engfunc() positional param type mismatch (reverse/forward 213) `[rules: engfunc_int_float, engfunc_float_int]`
`engfunc(type, any:...)` is fully variadic in fakemeta.inc, so amxxpc cannot type-check its
arguments at all — every EngFunc_* selector has its own fixed HLSDK signature (documented as
a comment on the enum member in fakemeta_const.inc) that the compiler never verifies. zplint
checks call-site literals positionally against a table built from those HLSDK signatures:
- `engfunc_int_float` (float literal into an int/entity slot): WalkMove/MoveToOrigin's mode arg,
  TraceLine/TraceHull/TraceModel/TraceSphere/TraceMonsterHull's hull/skip/flag args,
  Write(Byte|Char|Short|Long|Entity)'s value, MessageBegin's msg_dest/msg_type/ed.
- `engfunc_float_int` (int literal into a Float slot): WalkMove's yaw/dist, MoveToOrigin's dist,
  TraceSphere's radius, GetAimVector's speed, EmitSound/EmitAmbientSound's volume/attenuation,
  ParticleEffect's color/count, SetClientMaxspeed's speed, AnimationAutomove's flTime,
  CrosshairAngle's pitch/yaw, RunPlayerMove's forwardmove/sidemove/upmove,
  BuildSoundMsg's volume/attenuation, PlaybackEvent's delay/fparam1/fparam2,
  WriteCoord/WriteAngle's value. `FadeClientVolume`'s 4 args are ALL ints despite reading like
  durations - a real-world corpus hit confirmed `engfunc(EngFunc_WriteCoord, 36)` (bare int,
  reinterpreted as ~5e-44) instead of `36.0`.
Bare `0`/`-0` is exempt in both directions: its bit pattern is identical to `0.0`, so no bug.
Vector (`Float:x[3]`), string, and TraceResult-handle arguments are always passed as variables
in practice (never literals), so those positions are intentionally left unchecked.
Sources: https://raw.githubusercontent.com/alliedmodders/amxmodx/master/plugins/include/fakemeta.inc ·
https://raw.githubusercontent.com/alliedmodders/amxmodx/master/plugins/include/fakemeta_const.inc ·
https://wiki.alliedmods.net/Engine_Functions_(FAKEMETA) ·
https://wiki.alliedmods.net/Tags_(Scripting)

### 2.5c Engine module EV_* constant/native family mismatch `[rule: entity_ev_type_mismatch]`
The Engine module's `entity_get_*`/`entity_set_*` natives (engine.inc) are keyed by an `EV_*`
constant (engine_const.inc) whose prefix names the field's real storage type — `EV_INT_*` (36
constants: movetype, solid, effects, flags, team, ...), `EV_FL_*` (39: health, gravity, speed,
armorvalue, maxspeed, fuser1-4, ...), `EV_VEC_*` (23: origin, velocity, angles, mins/maxs, ...),
`EV_ENT_*` (11: owner, aiment, enemy, groundentity, ...), `EV_SZ_*` (13: classname, model,
target, ...), `EV_BYTE_*` (6: controller1-4, blending1-2). Neither the compiler nor the native
itself checks that the constant's family matches the function called — `entity_set_int(id,
EV_FL_gravity, 2)` silently writes an int bit pattern into a field the engine reads back as a
float (same failure mode as 2.4/2.5, just via the Engine module instead of fakemeta/pev). This
is exactly the AMXX scripting-tutorial's own "Advanced Topics" example
(`entity_set_int(player, EV_FL_armorvalue, value)`) — the prefix alone is enough to resolve the
correct native family, so the detector needs no per-constant table, just the 6 prefix->family
pairs. 0 hits (true or false) on both validation corpora — the API is simple enough that real
plugins get it right, but the check is essentially free and catches the copy-paste-wrong-EV_
case (e.g. copying an `EV_FL_` line and changing only the field name, forgetting the field
moved families).
Sources: https://raw.githubusercontent.com/alliedmodders/amxmodx/master/plugins/include/engine.inc ·
https://raw.githubusercontent.com/alliedmodders/amxmodx/master/plugins/include/engine_const.inc ·
https://www.amxmodx.org/doc/source/scripting/advanced.htm (section 8, "Entities")

### 2.5d Hamsandwich ExecuteHam(B) positional param type mismatch `[rules: ham_int_float, ham_float_int]`
Same failure class as 2.5b (`engfunc`), same root cause: `native ExecuteHam(Ham:function, this,
any:...)` is fully variadic (hamsandwich.inc), so amxxpc cannot type-check its arguments either.
Every `Ham_*` forward's real parameter list is documented as a "Forward params:" comment on its
enum member in ham_const.inc (~470 members total). Most are specific to other mods (TFC/NS/
SC/ESF/DOD/TS) and irrelevant to CS/ZP scripting, so the table is deliberately small and
curated to the handful actually seen in this domain: `Ham_TakeDamage` (this, idinflictor,
idattacker, Float:damage, damagebits), `Ham_TakeHealth` (this, Float:health, damagebits),
`Ham_TraceAttack` (this, idattacker, Float:damage, Float:direction[3], traceresult,
damagebits), `Ham_Use` (this, idcaller, idactivator, use_type, Float:value),
`Ham_CS_Player_Blind` (this, Float:blindTime, Float:duration, Float:holdTime, alpha),
`Ham_CS_Player_GetAutoaimVector` (this, Float:delta, Float:output[3] byref). Checked against
both `ExecuteHam()` and `ExecuteHamB()` (the "block" variant most ZP code actually uses for
`Ham_TakeDamage`). 0 hits on both validation corpora, including 26 real `ExecuteHamB(Ham_
TakeDamage, ...)` call sites in the 542-file corpus — all correctly typed (`25.0`, `float(dmg)`,
a `Float:`-tagged variable, or `get_pcvar_float(...)`), so this is a safety net rather than a
demonstrated bug source, same posture as 2.5c.
Sources: https://raw.githubusercontent.com/alliedmodders/amxmodx/master/plugins/include/hamsandwich.inc ·
https://raw.githubusercontent.com/alliedmodders/amxmodx/master/plugins/include/ham_const.inc

### 2.5e SetHamParam{Integer,Float}(which, ..) setter/slot mismatch `[rule: set_ham_param_mismatch]`
A different angle on the same Hamsandwich forwards from 2.5d: pre-hook code overrides an
incoming parameter via `SetHamParamFloat(which, ..)`/`SetHamParamInteger(which, ..)`, where
`which` is the 1-indexed position in the *forward's own* declared parameter list (`this` = 1).
Nothing cross-checks that the setter family (Integer vs Float) matches the real type of that
slot, so `SetHamParamInteger(4, 100)` inside a `Ham_TakeDamage` hook (slot 4 = `Float:damage`)
silently corrupts the damage value — same bit-reinterpretation bug as 2.5d, reached through a
different native pair. This check is stronger than 2.5d's: it flags on the setter function
chosen, not on whether the value happens to look like a literal, so a wrong setter is caught
even when called with a variable or expression. Traced by matching `RegisterHam(Ham_X, "class",
"callback")` registrations against `callback`'s body (reusing `find_function_body_in`, the same
scope-tracing helper `message_hook_scope`/`deathmsg_killer_guard` already use).
**Vector correction found via corpus validation:** a `Float:vec[3]` forward parameter consumes
**three consecutive** `which` slots (one per x/y/z component), not one — `Ham_TraceAttack`'s
`Float:direction[3]` (its 4th forward param) is addressed as which=4,5,6, shifting `traceresult`
to which=7 and `damagebits` to which=8. An initial version of this table treated the vector as
a single opaque slot and misread `SetHamParamFloat(5, direction[1] * resist)` /
`SetHamParamFloat(6, direction[2] * resist)` — real code from
`04-Complementos/zp50_addon_evolution.sma` — as writing into TraceAttack's (nonexistent, in
that numbering) int slots. Fixed with a second, which-expanded table (`HAM_WHICH_PARAM_TYPES`)
kept separate from 2.5d's `ExecuteHam`-args table (where the same vector is one call argument,
not three). 0 hits on both validation corpora after the fix.
Sources: https://raw.githubusercontent.com/alliedmodders/amxmodx/master/plugins/include/hamsandwich.inc ·
real-corpus regression case (`zp50_addon_evolution.sma`)

### 2.5f cstrike.inc int literal in Float parameter `[rule: cs_float_int]`
Same failure class as 2.4, applied to cstrike.inc's single-arg Float setters:
`cs_set_user_lastactivity`, `cs_set_hostage_lastuse`, `cs_set_hostage_nextuse`,
`cs_set_c4_explode_time` all take `(index, Float:value)`. A bare int literal (e.g.
`cs_set_c4_explode_time(id, 10)` instead of `10.0`) is bit-reinterpreted the same way as
`pev_health`. Literal `0`/`-0` is exempt (bit-identical to `0.0`). 0 hits on both corpora — none
of these four natives appear anywhere in the 542-file real-world collection, so this is a purely
preventive, zero-cost rule rather than a demonstrated bug source.
Sources: https://raw.githubusercontent.com/alliedmodders/amxmodx/master/plugins/include/cstrike.inc

### 2.5g fun.inc int literal in Float parameter `[rule: fun_float_int]`
`set_user_maxspeed(index, Float:speed)` / `set_user_gravity(index, Float:gravity)` - same bug
class as 2.4/2.5f, but for two of the most heavily used ZP class-scripting natives (hundreds of
call sites across the 542-file corpus, e.g. `set_user_gravity(id, 1)` instead of `1.0` would
silently set gravity to ~1.4e-45). 0 hits — every real call site sampled uses a `.0` literal,
a `Float:`-tagged variable, or `get_pcvar_float(...)`, so this is a safety net confirmed against
heavy real usage rather than a demonstrated bug source.
Sources: https://raw.githubusercontent.com/alliedmodders/amxmodx/master/plugins/include/fun.inc

### 2.5h amxmodx.inc positional param type mismatch `[rules: amxmodx_int_float, amxmodx_float_int]`
Unlike 2.5b/2.5d (`engfunc`/`ExecuteHam`, both fully variadic `any:...` dispatchers where the
compiler cannot type-check anything), `set_hudmessage`, `set_dhudmessage`, `emit_sound`, and
`change_task` are NORMALLY tagged natives — amxxpc already emits warning 213 for a mismatch
here, same as `set_task`'s interval. zplint duplicates that check ahead of compilation using a
positional type table, same mechanism as `engfunc_int_float`/`engfunc_float_int` but keyed
directly by function name (no dispatcher selector to skip, so table index 0 = the first real
argument): `set_hudmessage(r, g, b, Float:x, Float:y, effects, Float:fxtime, Float:holdtime,
Float:fadeintime, Float:fadeouttime, channel, alpha1, color2[4])`, `set_dhudmessage` (same
first 10 params), `emit_sound(index, channel, sample[], Float:vol, Float:att, flags, pitch)`,
`change_task(id, Float:newTime, outside)`. These are extremely common in ZP HUD/sound code
(hundreds of call sites across the real-world corpus) yet 0 hits — every sampled call site was
already correctly typed.
Sources: https://raw.githubusercontent.com/alliedmodders/amxmodx/master/plugins/include/amxmodx.inc

### 2.6 Unreachable code (warning 225) `[rule: unreachable_code]`
Statements after unconditional return/break/continue. Real case: amxmodx's own amxmisc.inc
had a trailing dead `return 0;` (fixed in issue #782).
Sources: https://github.com/alliedmodders/amxmodx/issues/782

### 2.7 Deprecated symbols (warning 233, AMXX 1.9) `[rule: deprecated_symbols]`
`client_disconnect` → `client_disconnected` (old forward misses aborted connections → state
leaks); `md5/md5_file` → `hash_string/hash_file`; `strbreak` → `argbreak`.
Sources: https://wiki.alliedmods.net/AMX_Mod_X_1.9_API_Changes

### 2.8 Redefining include constants (warning 201) `[rule: define_reserved_const]`
`#define MAX_PLAYERS 32` — already in amxconst.inc (1.8.3+). A different value silently
desynchronizes buffer sizes. Also: MAX_NAME_LENGTH, MAX_STRING_LENGTH, MAX_MOTD_LENGTH,
MAX_IP_LENGTH, MAX_AUTHID_LENGTH.
Sources: https://raw.githubusercontent.com/alliedmodders/amxmodx/master/plugins/include/amxconst.inc

### 2.9 Local shadows global (warning 219) `[rule: global_shadowing]`
`new g_count` inside a function — writes land in the local; "my global never updates".
Sources: sc5-in.scp · https://sampwiki.blast.hk/wiki/Errors_List

### 2.10 Constant conditions (warnings 205/206) `[rule: constant_condition]`
`if (0) { give_reward(id); }` — debugging leftover that dead-codes a feature.
Sources: sc5-in.scp

### 2.11 Others documented, not (yet) implemented
- 202 wrong arg count (needs in-file definition table; natives need include tables)
- 203/204 unused / write-only symbols (dead-code signal)
- 208 Float function used before definition (forces reparse)
- 209 mixed return paths — in AMXX callbacks a fall-off returns 0 = PLUGIN_CONTINUE /
  HAM_IGNORED, so an explicit-return-elsewhere callback silently doesn't block. High value,
  FP-prone; revisit with better return-path analysis.
- 217 loose indentation (mixed tabs/spaces) — cosmetic; users suppress with `#pragma tabsize 0`.

---

## 3. Runtime Errors

### 3.1 Run time error 4: index out of bounds
The most common AMXX runtime error (per official Debugging wiki).
- **[rule: userid_as_index]** `arr[get_user_userid(id)]` — userid is a session counter (500+),
  not a client index. Always a bug.
- **[rule: player_array_32]** `new arr[32]` indexed by `id` (player ids are 1..32; need [33] /
  MAX_PLAYERS+1). Fills up only on a full server — classic "crashes only under load".
- Entity index (create_entity/find_ent_by_class returns up to ~2048) into a [33] player array.
Sources: https://wiki.alliedmods.net/Debugging_Plugins_(AMX_Mod_X) ·
https://forums.alliedmods.net/showthread.php?t=150704 ·
https://www.amxmodx.org/api/amxmodx/get_user_userid

### 3.2 Run time error 3: stack error `[rule: pragma_dynamic_stack]`
Default AMX stack/heap = 16KB (4096 cells). Large locals (`new buf[4096]`) exhaust it.
Fix: `#pragma dynamic 32768`, or static/global buffers. Also caused by hook re-entry
recursion (a register_message handler emitting the same message; ExecuteHam of the hooked
Ham inside its own hook).
Sources: http://www.amxmodx.org/doc/source/scripting/debug.htm ·
https://forums.alliedmods.net/archive/index.php/t-318486.html

### 3.3 Run time error 10: native error
- pev/set_pev/entity_* on stale/unvalidated entity ("[FAKEMETA] Invalid entity") — Think
  hooks and set_task callbacks firing after the entity is gone; guard with `pev_valid()`.
- Player natives on dead/HLTV/unconnected players — `get_players()` without "a"/"h" flags
  feeding set_user_health etc.
- ZP50 natives validate and log "[ZP] Invalid Player (%d)" (see §6).
Sources: https://www.amxmodx.org/api/fakemeta/pev · https://www.amxmodx.org/api/amxmodx/get_players ·
https://amxx.pl/topic/148293-fakemeta-invalid-entity/

### 3.4 Run time error 11: divide by zero `[rule: div_by_runtime]`
`x / get_playersnum()` on empty server; `/ get_pcvar_num(p)` when admin sets cvar 0.
Sources: https://github.com/alliedmodders/amxmodx/blob/master/amxmodx/debugger.cpp ·
https://forums.alliedmods.net/showthread.php?t=190020

### 3.5 Infinite loop / server freeze `[rule: find_ent_no_advance]`
`while ((ent = find_ent_by_class(-1, "cls")))` — passing a constant start index returns the
same entity forever; GoldSrc has no watchdog, the server hard-freezes at 100% CPU. The start
argument must be the previously found entity.
Sources: https://forums.alliedmods.net/showthread.php?t=77761 ·
https://www.amxmodx.org/api/engine/__functions

### 3.6 DeathMsg killer id can be 0 `[rule: deathmsg_killer_guard]`
"KillerID would be 0 if a player died from fall/acid/radiation" — `read_data(1)` into
`g_kills[killer]++` or get_user_name without a zero/connected check → wrong behavior or
error 4.
Sources: https://wiki.alliedmods.net/Half-Life_1_Game_Events

### 3.7 Format-string injection `[rule: format_injection]`
`client_print(0, print_chat, said)` where `said` is user text: any `%` in a nickname or chat
becomes a format specifier (crash/garbage; amxmodx changelog records format crash fixes).
Always pin a literal: `client_print(0, print_chat, "%s", said)`.
Sources: https://www.amxmodx.org/api/amxmodx/client_print ·
https://github.com/alliedmodders/amxmodx/issues/776

### 3.8 Synchronous remove_entity in a damage hook `[rule: remove_entity_in_damage_hook]`
`remove_entity(ent)` called directly inside a `Ham_TakeDamage` callback frees the edict
mid-`FireBullets`; a multi-pellet weapon (shotgun/multiple hits in the same frame) then
dereferences the freed edict on the next pellet → segfault / server freeze. `is_valid_ent`
at the top does not help — the engine still holds the pointer. Fix: neutralize in-frame
(dying flag + DAMAGE_NO + SOLID_NOT + EF_NODRAW so later pellets no-op) and defer the real
`remove_entity` via `set_task`, or set `pev_flags | FL_KILLME` (engine removes next frame).
Scoped to Ham_TakeDamage only — self-removing pickups in Ham_Touch are the ubiquitous safe
idiom and are intentionally not flagged.
Confirmed in this repo: zp50_atmospheric_headcrabs (shoot headcrab → freeze), and latent in
zp50_extra_headcrab / zp50_class_overlord.

---

## 4. Engine / HLDS Pitfalls

### 4.1 Precache
- 512-entry hard limit per table (models, sounds): over it → fatal
  "Host_Error: PF_precache_model_I: ... over the 512 limit" at map start.
  `[rule: precache_in_loop]` flags precache_* inside loops (config-driven lists).
- `.mp3` cannot go through precache_sound/emit_sound — precache_generic + 
  `client_cmd "mp3 play"` only. `[rule: precache_mp3]`
- Paths are relative to `sound/`: `precache_sound("sound/x.wav")` looks up sound/sound/x.wav
  and never plays. (precache_generic DOES need the `sound/` prefix — opposite convention.)
  `[rule: sound_prefix]`
- Model set via entity_set_model/EngFunc_SetModel/pev_model with a literal path never
  precached in the file → fatal "SV_ModelIndex: model not precached". `[rule: model_not_precached]`
Sources: https://github.com/dreamstalker/rehlds/issues/633 ·
https://www.amxmodx.org/api/amxmodx/precache_sound · https://forums.alliedmods.net/showthread.php?t=343221

### 4.2 Edict pool exhaustion `[rule: entity_leak]`
Fixed pool (~900 default, 2048 max). create_entity per event/kill/frame without a
remove_entity path → accumulates until fatal "ED_Alloc: no free edicts".
Sources: https://www.moddb.com/tutorials/fixing-ed-alloc-no-free-edicts ·
https://forums.alliedmods.net/showthread.php?t=312475

### 4.3 Network overflow `[rule: te_reliable]`
SVC_TEMPENTITY belongs on the unreliable datagram (MSG_BROADCAST/MSG_PVS/MSG_ONE_UNRELIABLE).
MSG_ALL/MSG_ONE force the reliable channel; per-frame/per-hit emission overflows the 4KB
netchan → "Reliable channel overflowed", players kicked.
Sources: https://forums.alliedmods.net/showthread.php?t=331372 · https://www.amxmodx.org/api/message_const

### 4.4 GoldSrc client command filter `[rule: mp3_loading_path]`
The client stufftext filter rejects commands containing the substring "loading" —
`client_cmd(id, "mp3 play sound/x/loading/1.mp3")` → "Server tried to send invalid command"
on every client. Confirmed: amxmodx issue #818.
Sources: https://github.com/alliedmodders/amxmodx/issues/818

### 4.5 String hunk exhaustion (EngFunc_AllocString)
Allocates from the map hunk on EVERY call (even identical strings), freed only on map change.
Per-frame use (custom weapon viewmodels in PreThink) → "Hunk_Alloc: failed" crash. Cache the
handle in a static. (Deferred: needs hook-registration correlation to stay FP-safe.)
Sources: https://forums.alliedmods.net/archive/index.php/t-299492.html

### 4.6 HUD channels `[rule: hud_channel_range]`
Clients have exactly 4 HUD text channels; set_hudmessage channel must be 1–4 or -1 (auto).
Literal >4 stomps another channel unpredictably.
Sources: https://www.amxmodx.org/api/amxmodx/set_hudmessage

### 4.7 changelevel via server_cmd `[rule: changelevel_cmd]`
Bypasses the server_changelevel forward and Metamod hooks (map-manager plugins never see it),
and an invalid map string errors the server. Use `is_map_valid()` + `change_level()`.
Sources: https://www.amxmodx.org/api/amxmodx/change_level

### 4.8 Forward return contracts
- client_command: PLUGIN_HANDLED stops OTHER plugins' handlers too; amxconst.inc defines
  PLUGIN_HANDLED_MAIN specifically for "stop command, continue plugins". `[rule: client_command_handled]`
- say/say_team hooks ending in unconditional PLUGIN_HANDLED eat all server chat.
- Fakemeta register_forward callbacks must return FMRES_*; fall-off returns 0 (invalid).
Sources: https://github.com/alliedmodders/amxmodx/blob/master/plugins/include/amxconst.inc ·
https://wiki.alliedmods.net/FakeMeta_General_Usage_(AMX_Mod_X)

### 4.9 client_connect is too early `[rule: client_connect_actions]`
Official docs: "called too early to do anything that directly affects the client."
client_print/set_user_*/show_menu there silently no-op or throw error 10. Use
client_putinserver.
Sources: https://www.amxmodx.org/api/amxmodx/client_connect

### 4.10 geoip_code2/3 one-cell buffer overflow `[rule: geoip_code_overflow]`
geoip.inc documents that the (non-`_ex`) `geoip_code2`/`geoip_code3` natives overflow their
result buffer by one cell on an unknown IP — a native-implementation defect, not a caller
mistake, but one with a trivial fix: use `geoip_code2_ex`/`geoip_code3_ex` instead. Extremely
low real-world reach (only 1 call site in the 542-file corpus, already using the safe `_ex`
form), but the check is a single regex line, so the cost is effectively zero.
Sources: https://raw.githubusercontent.com/alliedmodders/amxmodx/master/plugins/include/geoip.inc

---

## 5. Performance (documented anti-patterns, Optimizing Plugins wiki)

- `[rule: strlen_in_loop]` strlen() in for-condition → recomputed every iteration, O(n²).
- `[rule: get_cvar_hotpath]` get_cvar_num/float/string outside init — string hash lookup per
  call; pcvar access is "dozens of times faster". Cache register_cvar() handle, use get_pcvar_*.
- `[rule: buffer_in_loop]` `new buf[N]` inside a loop body — re-zeroed every iteration
  ("32 players → resize and zero over 1K of memory 32 times").
- `[rule: read_file_loop]` read_file/write_file are O(n²) for consecutive I/O (reopen + rescan
  per call); fopen/fgets/fclose are O(n).
- Repeated pure native call in if/else-if chain (compiler never caches native results).
- Same hardcoded string literal duplicated — compiler does not dedup literals in DATA.
- cs_set_user_model per frame is redundant (module re-applies automatically) and legacy
  userinfo-based model change per frame historically caused svc_bad kicks.
Sources: https://wiki.alliedmods.net/Optimizing_Plugins_(AMX_Mod_X_Scripting) ·
https://www.amxmodx.org/api/cstrike/cs_set_user_model

---

## 6. ZP 5.0 (zp50) API Contracts

Verified against official zp50 sources (zp50_core.sma, zp50_items.sma, item/class/gamemode
plugins, includes) and the 5.0.8a changelog.

### 6.1 Player validity — "[ZP] Invalid Player (%d)"
Every player-taking zp50 native validates is_user_connected (infect/cure also is_user_alive)
and raises AMX_ERR_NATIVE (run time error 10) on failure. Official changelog 5.0.8a records
exactly this bug class: "Fixed run time error '[ZP] Invalid Player' (Extra item: Infection
Bomb)" — fixed by adding `is_user_alive(victim)` guards before zp_core_infect in delayed
callbacks. Delayed contexts (set_task, DeathMsg, disconnect) are the classic offenders.
Also: zp_core_infect on an already-zombie → "[ZP] Player already infected"; zp_core_cure on
a non-zombie → "[ZP] Player not infected". (zplint rules: zp_infect_cure_guard,
zp_force_no_guard, existing; extended native list includes zp_ammopacks_get/set.)
Sources: zp50_core.sma + zp50_changelog.txt (5.0.8a) via
https://raw.githubusercontent.com/evandrocoan/MultiModServer/master/plugins/addons/amxmodx/scripting/zp50_core.sma ·
https://raw.githubusercontent.com/Sh1ft0x0EF/Zombie-Plague-Mod/5.0.8a/zp50_changelog.txt

### 6.2 Forward `attacker` can be 0 `[rule: zp_fw_attacker_guard]`
zp50_core.inc: zp_fw_core_infect(id, attacker) / zp_fw_core_cure(id, attacker) —
"attacker index (0 if not available)". First zombie of the round, admin and console
infections all deliver attacker=0. Reward plugins calling zp_ammopacks_set(attacker, ...)
without a guard error every round start.
Sources: https://github.com/CSnajper/zm_exp_scripting/blob/master/include/zp50_core.inc

### 6.3 Registration must happen in plugin_precache `[rule: zp_class_in_init — extended]`
All official content plugins register in plugin_precache(): zp_class_zombie_register(+_model/
_claw/_kb), zp_class_human_register(+_model), zp_gamemodes_register, zp_gamemodes_set_default.
Model registration precaches immediately → calling from plugin_init is late-precache (fatal).
Late class/gamemode registration also breaks the core's settings-file pass.
Sources: zp50_class_zombie_leech.sma · zp50_gamemode_infection.sma (official sources)

### 6.4 Registration ids must be stored `[rule: zp50_register_return]`
zp_items_register / zp_class_*_register / zp_gamemodes_register return the id that is the
ONLY handle for filtering later forwards (or ZP_INVALID_* = -1). Discarding it → the
"handler reacts to every item" bug.
Sources: zp50_items_const.inc · zp50_class_zombie.inc

### 6.5 select_pre forwards take max across ALL plugins `[rule: zp_select_pre_filter]`
zp50_items.sma: `if (g_ForwardResult >= ZP_ITEM_DONT_SHOW)` — the HIGHEST return across all
subplugins wins for EVERY item. A handler returning ZP_ITEM_NOT_AVAILABLE/DONT_SHOW without
first checking `itemid != g_ItemID` blocks/hides every item server-wide (the classic
"[ZP] No extra items are currently available" complaint). Official pattern (infection bomb):
`if (itemid != g_ItemID) return ZP_ITEM_AVAILABLE;` as the first line. Analogous for
zp_fw_class_*_select_pre and zp_fw_gamemodes_choose_pre.
Also `[rule: zp_select_pre_return]`: these forwards use ZP_ITEM_*/ZP_CLASS_* constants —
returning PLUGIN_HANDLED (=1) accidentally means NOT_AVAILABLE; conversely
zp_fw_core_infect_pre/cure_pre/gamemodes_choose_pre are blocked with PLUGIN_HANDLED, not ZP_*.
Sources: zp50_items.sma · zp50_item_infection_bomb.sma · zp50_items_const.inc

### 6.6 Other zp50 contracts (documented, lower priority)
- zp_fw_core_last_zombie also fires for the FIRST zombie (include doc) — last-zombie buffs
  need zp_core_is_first_zombie/zombie_count checks.
- Unconditional PLUGIN_HANDLED in zp_fw_core_infect_pre deadlocks round start (first zombie
  can never be made).
- zp_*_menu_text_add only meaningful inside the matching select_pre forward.
- Mixing ZP 4.3 API (`#include <zombieplague>`, zp_get_user_zombie, zp_register_extra_item)
  with zp50 includes → "missing natives" load failure without the compat addon. `[rule: zp43_mixing]`
- Query natives (zp_*_get_id/get_count) in plugin_init → "Invalid Array Handle" from plugin
  load order (fixed class of bugs in ZP 5.0.6 changelog); use plugin_cfg or forwards.
  `[rule: zp50_get_in_init]`
- zp_class_zombie_get_max_health(id, classid) takes PLAYER first — 4.3 porters pass classid
  alone and hit "[ZP] Invalid Player".
- Infecting the last human instead of killing him stalls round end (official infection bomb
  special-cases `zp_core_get_human_count() == 1` → Ham_Killed).
Sources: zp50_core.inc · zp50_changelog.txt · zp50_item_infection_bomb.sma ·
https://forums.alliedmods.net/archive/index.php/t-214127.html

---

## 7. Correctness idioms (string API)

- `[rule: contain_truthy]` contain()/containi() return position or **-1 on miss** —
  `if (contain(msg, "x"))` treats not-found as true and found-at-0 as false. Compare `!= -1`.
- `[rule: strcmp_truthy]` strcmp() returns 0 on match — bare `if (strcmp(a,b))` means
  "if different". `!strcmp(...)` is idiomatic (don't flag); suggest equal() or `== 0`.
- `[rule: formatex_self]` formatex() skips copy-back checking — using the output buffer as a
  %s input produces corrupted output; format() handles overlap.
Sources: https://www.amxmodx.org/api/string ·
https://wiki.alliedmods.net/Optimizing_Plugins_(AMX_Mod_X_Scripting)

- `[rule: sql_fieldname_truthy]` sqlx.inc's `SQL_FieldNameToNum(query, name)` returns **-1
  on failure**, but column indices are 0-based, so `if (SQL_FieldNameToNum(query, "id"))`
  misreads a valid match on column 0 as failure — same truthy-return-value bug class as
  `contain_truthy`/`strcmp_truthy` above, found while checking sqlx.inc for the Float/int
  mismatch class (that file has none — all-string/handle natives). 5 real call sites in the
  542-file corpus (`admin.sma`, `mysql.inc`) all assign the result to a variable first
  (`new qcolAuth = SQL_FieldNameToNum(...)`) rather than testing it bare, so 0 hits — a
  preventive rule for the one idiom that would actually be wrong, not a demonstrated bug source.
Sources: https://raw.githubusercontent.com/alliedmodders/amxmodx/master/plugins/include/sqlx.inc

- `[rule: func_id_truthy]` amxmodx.inc's `get_func_id()`/`get_xvar_id()` return **-1 on
  failure**, but their docs explicitly state valid ids are `>=0` — id 0 is real and falsy, same
  bug class as the two rules above. Checks both the direct-call form (`if (get_xvar_id(..))`)
  and the far more common real idiom, an intermediate variable tested a line or two later
  (`new x = get_xvar_id(name); if (x) ...`), traced via `enclosing_body()` the same way
  `zp_fw_attacker_guard`/`zp_select_pre_filter` already trace variable usage across a function.
  **Confirmed as a real, live bug, not just a theoretical risk:** `plmenu.sma` — bundled with
  the official `alliedmodders/amxmodx` distribution itself (present under `plugins/`, `plugins/
  dod/`, `plugins/ns/`, `plugins/tfc/` — 4 copies, all identical) and separately present in the
  542-file real-world ZP corpus — contains exactly this bug at `new x = get_xvar_id("g_temp
  Bans"); if (x)`. Severity is `warning` (matching its sibling truthy rules), which is also why
  this was initially miscategorized as `error` and technically broke the "0 errors on official
  plugins" baseline gate until fixed — a good reminder to add every new rule to both the
  detector table and `WARNING_RULES` in the same change when it belongs there.
Sources: https://raw.githubusercontent.com/alliedmodders/amxmodx/master/plugins/include/amxmodx.inc ·
real-corpus + official-bundled-plugin regression case (`plmenu.sma`)

---

## 8. Multilingual (%L) notes

Each %L consumes target + key. LANG_PLAYER is for broadcasts (per-player language);
LANG_SERVER on a broadcast forces server language on everyone; missing target shifts all
arguments (garbage/runtime error). (Deferred detector: needs argument counting per format.)
Sources: https://wiki.alliedmods.net/Advanced_Scripting_(AMX_Mod_X)
