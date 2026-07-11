# zplint 🔍

Lightning-fast linter for **Zombie Plague 5.0** AMXX plugins (CS 1.6).  
Scans `.sma` files for **27 runtime-crash patterns** that cause HLDS `svc_bad` / segfault / run time error 10.

Written in **Rust**. Single binary, zero runtime dependencies. ~0.7s for 300 files.

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
zplint watch             # Re-lint on file save
zplint fix               # Apply auto-fixes
```

## Rules (27 detectors)

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

### Entity & Memory (4)
| Rule | Severity | Fix | Description |
|------|----------|-----|-------------|
| `create_entity_guard` | error | ❌ | create_entity without is_valid_ent check |
| `nested_message` | error | ❌ | Nested message_begin without message_end (crashes server) |
| `touch_spam` | warning | ❌ | Touch handler spams client_print |
| `precache_sound` | error | ❌ | emit_sound with custom sound never precached |

### ZP API Misuse (6)
| Rule | Severity | Fix | Description |
|------|----------|-----|-------------|
| `zp_gamemode_if` | error | ✅ | `if (mode)` should be `if (mode > 0)` |
| `zp_class_if` | error | ✅ | `if (class)` should be `if (class > 0)` |
| `zp_items_register_check` | error | ❌ | zp_items_register() return value not checked |
| `precache_outside_precache` | error | ❌ | precache_*() called in plugin_init() not plugin_precache() -> crash |
| `class_register_in_init` | error | ❌ | zp_class_*_register() in plugin_init() instead of plugin_precache() |
| `library_exists_hotpath` | warning | ❌ | LibraryExists() per-hit in TakeDamage (cache as bool) |

### AMXX General (5)
| Rule | Severity | Fix | Description |
|------|----------|-----|-------------|
| `set_task_public` | error | ❌ | set_task callback is non-public function |
| `read_data_multi_context` | error | ❌ | read_data() in event + non-event callback |
| `pev_oldbuttons` | warning | ❌ | pev_oldbuttons unreliable in PreThink |
| `get_user_origin` | warning | ❌ | get_user_origin() loses float precision |
| `task_interval_zero` | error | ❌ | set_task with interval 0.0 (minimum 0.1) |

### Code Smells (4)
| Rule | Severity | Fix | Description |
|------|----------|-----|-------------|
| `abort_call` | error | ❌ | abort() causes run time error 1 |
| `precache_sound_sprite` | error | ❌ | precache_sound on sprite-named variable (use precache_model) |
| `buffer_size` | error | ✅ | Hardcoded buffer < 64 in get_user_* (use charsmax) |
| `client_cmd_spk` | error | ❌ | client_cmd(0, "spk...") instead of emit_sound |
| `hardcoded_maxplayers` | warning | ❌ | #define MAXPLAYERS 32 (use get_maxplayers()) |

## Config

`zplint.toml` in project root:

```toml
[lint]
paths = ["meus_plugins_organizados"]
exclude = ["00-Old_Archive"]

[lint.rules]
# true = enable, false = disable
client_disconnect_guard = true
zp_gamemode_if = true
abort_call = false
# ...

[output]
color = true
```

## Performance

~300 `.sma` files = **0.74s** (release build).

## License

MIT
