# zplint 🔍

Lightning-fast linter for **Pawn / AMX Mod X** plugins (CS 1.6), with deep **Zombie Plague 5.0** support.  
Scans `.sma` files for **106 detectors**: compile errors before you compile, runtime-crash patterns
(HLDS `svc_bad` / segfault / run time errors 3/4/10/11), engine limits (precache/edicts/netchan),
tag-mismatch bugs, and ZP50 API contract violations.

Every rule is backed by a documented source (AMXX compiler sources, AlliedModders wiki/forums,
amxmodx.org API docs, official ZP 5.0 sources) — see [`docs/KNOWLEDGE.md`](docs/KNOWLEDGE.md).

Written in **Rust**, parallelized with rayon. Single binary, zero runtime dependencies.
~1.7s for 542 files. Reads Windows-1252 legacy files, honors `#pragma ctrlchar`.

Validated against two corpora: the official `alliedmodders/amxmodx` bundled plugins
(**0 errors** — canonical code passes clean) and a 542-file real-world ZP plugin collection.

## Install

```bash
git clone https://github.com/your/zplint
cd zplint
cargo build --release
./target/release/zplint --help
```

## Usage

```bash
zplint lint              # Lint all .sma files
zplint lint file.sma     # Lint specific file
zplint lint folder/      # Lint .sma files under a folder
zplint watch             # Re-lint on file save
zplint fix               # Apply auto-fixes
```

## Rules (106 detectors)

### Player Validation (8)
| Rule | Severity | Fix | Description |
|------|----------|-----|-------------|
| `client_disconnect_guard` | error | ❌ | Risky natives in client_disconnected without guard |
| `dangerous_forward_guard` | error | ❌ | Risky natives in fw_Killed/Ham_Killed/fw_Spawn |
| `message_begin_guard` | error | ❌ | message_begin(MSG_ONE) with unvalidated index -> `svc_bad` |
| `find_entity_in_sphere` | error | ❌ | FindEntityInSphere result used as player |
| `loop_player_guard` | error | ❌ | 1-32 loop without is_user_connected guard |
| `attacker_not_validated` | error | ❌ | TakeDamage handler uses attacker without is_user_alive (attacker = 0/world) |
| `zp_infect_cure_guard` | error | ❌ | zp_core_infect/cure without is_zombie check |
| `zp_force_no_guard` | error | ❌ | zp_core_force_infect/cure bypasses validation |

### Entity & Memory (9)
| Rule | Severity | Fix | Description |
|------|----------|-----|-------------|
| `create_entity_guard` | error | ❌ | create_entity without is_valid_ent check |
| `nested_message` | error | ❌ | Nested message_begin without message_end (crashes server) |
| `message_write_outside` | error | ❌ | write_* outside message_begin/message_end (crashes server) |
| `message_end_without_begin` | error | ❌ | message_end without message_begin (crashes server) |
| `message_hook_scope` | error | ❌ | get_msg_arg*/set_msg_arg* outside register_message callback |
| `hardcoded_message_id` | error | ❌ | numeric message id in message_begin (use get_user_msgid/constant) |
| `array_random_empty` | error | ❌ | ArrayGet* with random ArraySize()-1 without non-empty guard |
| `touch_spam` | warning | ❌ | Touch handler spams client_print |
| `precache_sound` | error | ❌ | emit_sound with custom sound never precached |

### ZP API Misuse (5)
| Rule | Severity | Fix | Description |
|------|----------|-----|-------------|
| `zp_gamemode_if` | error | ✅ | `if (mode)` should be `if (mode > 0)` |
| `zp_class_if` | error | ✅ | `if (class)` should be `if (class > 0)`; also checks `zp_class_*_register()` in wrong scope |
| `zp_items_register_check` | error | ❌ | zp_items_register() return value not checked |
| `precache_outside_precache` | error | ❌ | precache_*() called in plugin_init() not plugin_precache() -> crash |
| `library_exists_hotpath` | warning | ❌ | LibraryExists() per-hit in TakeDamage (cache as bool) |

### AMXX General (10)
| Rule | Severity | Fix | Description |
|------|----------|-----|-------------|
| `set_task_public` | error | ❌ | set_task callback is non-public function |
| `registered_callback_public` | error | ❌ | menu/register callback is non-public function |
| `read_data_multi_context` | error | ❌ | read_data() in event + non-event callback |
| `pev_oldbuttons` | warning | ❌ | pev_oldbuttons unreliable in PreThink |
| `get_user_origin` | warning | ❌ | get_user_origin() loses float precision |
| `task_interval_zero` | error | ❌ | set_task with interval 0.0 (minimum 0.1) |
| `set_task_flags` | error | ❌ | set_task invalid flags/repeat usage |
| `percent_n_player_name` | error | ❌ | `%n` player-name formatter can throw on invalid index |
| `menu_handler_destroy` | error | ❌ | dynamic menu_create handler does not call menu_destroy |
| `fopen_close` | error | ❌ | fopen handle is not closed with fclose in same function |

### Code Smells (5)
| Rule | Severity | Fix | Description |
|------|----------|-----|-------------|
| `abort_call` | error | ❌ | abort() causes run time error 1 |
| `precache_sound_sprite` | error | ❌ | precache_sound on sprite-named variable (use precache_model) |
| `buffer_size` | warning | ✅ | Hardcoded buffer < 64 in get_user_* (use charsmax) |
| `client_cmd_spk` | warning | ❌ | client_cmd(0, "spk...") instead of emit_sound |
| `hardcoded_maxplayers` | warning | ❌ | loop uses `#define MAXPLAYERS 32` as runtime player count |

## Research-driven detectors (69)

Added from internet research of real compile errors, crash reports and API docs
(sources in [`docs/KNOWLEDGE.md`](docs/KNOWLEDGE.md)). All are on by default; turn any off
via `rules.disable` (see Config).

### Compile structure (9)
| Rule | Severity | Description |
|------|----------|-------------|
| `unbalanced_braces` | error | File ends with unmatched `{`/`}` (errors 030/054, cascades into 010/004) |
| `unbalanced_preprocessor` | error | `#if`/`#else`/`#endif` stack errors (026/060/061) |
| `unterminated_string` | error | Odd quote count on a line (error 037); escape-char and continuation aware |
| `else_paren` | error | `else (cond)` instead of `else if (cond)` (error 029/010) |
| `empty_statement` | error | `if (...)`/`while (...)` terminated by `;` detaches the block (error 036) |
| `stacked_case` | error | `case A:` `case B:` — Pawn has no fallthrough; use `case A, B:` |
| `line_too_long` | warning | Line > 511 chars (error 075 on amxxpc 1.8.x) |
| `array_index_oob` | error | Literal out-of-bounds array write, e.g. `Players[32] = 15` on a 32-slot array (valid 0-31) |
| `array_compare_by_ref` | error | `arr1 == arr2` compares array references, not contents — doesn't compile in Pawn |

### Correctness (14)
| Rule | Severity | Description |
|------|----------|-------------|
| `string_literal_compare` | error | `== "..."` — strings need equal()/equali() (error 033) |
| `string_assign` | warning | String literal larger than the destination array (error 047) |
| `forward_arity` | error | Known forward defined with wrong parameter count (error 025) |
| `formatex_self` | error | formatex() output buffer also used as input (corrupts output) |
| `assignment_in_condition` | warning | `if (x = 1)` (warning 211); `((x = y))` idiom is allowed |
| `comparison_as_statement` | warning | `x == 5;` as a statement does nothing (warning 215) |
| `self_assignment` | warning | `x = x;` (warning 226) |
| `constant_condition` | warning | `if (0)` / `if (1)` dead-codes a branch (warnings 205/206) |
| `unreachable_code` | warning | Statements after an unconditional return (warning 225) |
| `contain_truthy` | warning | contain() returns -1 on miss — bare truthiness inverts the logic |
| `strcmp_truthy` | warning | strcmp() returns 0 on match — bare `if (strcmp(..))` means "differs" |
| `sql_fieldname_truthy` | warning | `SQL_FieldNameToNum()` returns -1 on miss — bare truthiness misreads column 0 as failure |
| `func_id_truthy` | warning | `get_func_id()`/`get_xvar_id()` return -1 on failure but id 0 is valid — bare truthiness (direct or via variable) misreads it as failure |
| `global_shadowing` | warning | Local `new` shadows a global (warning 219) |

### Tag mismatch — warning 213 that misbehaves at runtime (13)
| Rule | Severity | Description |
|------|----------|-------------|
| `set_task_int_interval` | error | `set_task(10, ..)` → interval becomes ~1e-44s (runs every frame) |
| `pev_float_int` | error | Integer into Float pev field (pev_health 100 → ~1.4e-43 = instant death) |
| `int_native_float` | error | Float into int native (set_user_health(id, 100.0) → 1120403456 hp) |
| `engfunc_int_float` | error | Float literal into an engfunc() int/entity slot, checked positionally across ~20 EngFunc_* signatures (WalkMove mode, hull, WriteByte value, ...) |
| `engfunc_float_int` | error | Int literal into an engfunc() Float slot, same table, reverse direction (e.g. `EngFunc_WriteCoord(36)` instead of `36.0`) |
| `entity_ev_type_mismatch` | error | Engine module `entity_set_int(id, EV_FL_gravity, ..)` — EV_* prefix names the real family (int/float/vector/edict/string/byte), function used doesn't match |
| `ham_int_float` | error | Float literal into an ExecuteHam(B)/Ham_* int slot (e.g. Ham_Use's use_type) |
| `ham_float_int` | error | Int literal into an ExecuteHam(B)/Ham_* Float slot (e.g. `Ham_TakeDamage` damage passed as `25` instead of `25.0`) |
| `set_ham_param_mismatch` | error | `SetHamParamInteger(4, ..)` inside a `Ham_TakeDamage` hook — slot 4 is `Float:damage`, wrong setter used regardless of the value passed |
| `cs_float_int` | error | Int literal into a cstrike.inc Float setter (`cs_set_c4_explode_time`, `cs_set_user_lastactivity`, `cs_set_hostage_lastuse/nextuse`) |
| `fun_float_int` | error | Int literal into `set_user_maxspeed`/`set_user_gravity`'s Float parameter |
| `amxmodx_int_float` | error | Float literal into a `set_hudmessage`/`set_dhudmessage`/`emit_sound`/`change_task` int slot |
| `amxmodx_float_int` | error | Int literal into a `set_hudmessage`/`set_dhudmessage`/`emit_sound`/`change_task` Float slot (fxtime, holdtime, vol, att, newTime, ...) |

### Runtime crashes (7)
| Rule | Severity | Description |
|------|----------|-------------|
| `userid_as_index` | error | `arr[get_user_userid(id)]` — userid is a session counter, not an index (RTE 4) |
| `player_array_32` | error | `new arr[32]` indexed by player id — overflows on full server (RTE 4) |
| `find_ent_no_advance` | error | Entity-search loop restarting from a constant — infinite loop, server freeze |
| `deathmsg_killer_guard` | error | DeathMsg `read_data(1)` can be 0 (world kills) — guard before use |
| `div_by_runtime` | warning | Division by get_playersnum()/cvar that can be zero (RTE 11) |
| `pragma_dynamic_stack` | warning | Local array ≥ 2048 cells without `#pragma dynamic` (RTE 3) |
| `format_injection` | warning | User text (read_args/get_user_name) as format string — `%` in chat crashes |

### Engine / HLDS limits (10)
| Rule | Severity | Description |
|------|----------|-------------|
| `precache_mp3` | error | .mp3 through precache_sound/emit_sound — never plays; use precache_generic |
| `sound_prefix` | error | `"sound/..."` prefix — paths are already relative to sound/ |
| `model_not_precached` | error | Literal model set but never precached (fatal SV_ModelIndex) |
| `mp3_loading_path` | warning | Client stufftext filter silently blocks paths containing "loading" |
| `te_reliable` | warning | SVC_TEMPENTITY on MSG_ALL/MSG_ONE — reliable-channel overflow kicks |
| `precache_in_loop` | warning | precache_* inside a loop risks the 512-entry engine limit |
| `entity_leak` | warning | create_entity with no removal path — "ED_Alloc: no free edicts" |
| `hud_channel_range` | warning | set_hudmessage channel outside 1-4/-1 |
| `changelevel_cmd` | warning | server_cmd changelevel bypasses forwards and map validity check |
| `geoip_code_overflow` | error | `geoip_code2/3()` overflow their result buffer by one cell on an unknown IP — use the `_ex` variant |

### API contracts & modernization (8)
| Rule | Severity | Description |
|------|----------|-------------|
| `callback_not_defined` | warning | Registered callback string has no function in the file ("function not found") |
| `client_command_handled` | warning | PLUGIN_HANDLED in client_command starves other plugins (use _MAIN) |
| `client_connect_actions` | warning | Client-affecting natives in client_connect (too early; use putinserver) |
| `deprecated_symbols` | warning | client_disconnect / md5 / strbreak (AMXX 1.9 warning 233) |
| `define_reserved_const` | warning | Redefining MAX_PLAYERS etc. from amxconst.inc (warning 201) |
| `get_cvar_hotpath` | warning | get_cvar_* outside init — use cached pcvars ("dozens of times faster") |
| `strlen_in_loop` | warning | strlen() in loop condition — O(n²) |
| `buffer_in_loop` / `read_file_loop` | warning | Array re-zeroed per iteration / O(n²) file API in loops |

### ZP 5.0 API (6)
| Rule | Severity | Description |
|------|----------|-------------|
| `zp_fw_attacker_guard` | error | zp_fw_core_infect/cure `attacker` is 0 for gamemode/admin infections |
| `zp_select_pre_filter` | warning | select_pre returns restrictive ZP_* without using itemid/classid — blocks ALL items |
| `zp_select_pre_return` | warning | PLUGIN_HANDLED in select_pre (=NOT_AVAILABLE) / ZP_* in core _pre forwards |
| `zp50_register_return` | warning | Registration id discarded — cannot filter forwards for your item/class |
| `zp50_get_in_init` | warning | zp50 query natives in plugin_init ("Invalid Array Handle" load-order bug) |
| `zp43_mixing` | warning | ZP 4.3 API (`<zombieplague>`) mixed with zp50 includes |

## Config

`zplint.toml` in project root:

```toml
[lint]
paths = ["meus_plugins_organizados"]
exclude = ["00-Old_Archive"]

[lint.rules]
# original detectors: true = enable, false = disable
client_disconnect_guard = true
zp_gamemode_if = true
abort_call = false
# research-driven detectors are on by default; turn off by id:
disable = ["deprecated_symbols", "get_cvar_hotpath"]

[output]
color = true
```

Severity model: **errors** are crash/compile-failure patterns and set exit code 1;
**warnings** are style/perf/modernization signals and never fail CI.

## Performance

542 real `.sma` files = **1.7s**; official amxmodx plugins (74 files) = **0.35s** (release build, rayon-parallel).
Non-UTF8 (Windows-1252) legacy files are decoded, not skipped.

## License

MIT
