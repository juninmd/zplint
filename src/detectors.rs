//! Detectors sourced from web research (see docs/KNOWLEDGE.md).
//! All rules here are on by default and can be turned off via `rules.disable`.

use crate::config::RulesConfig;
use crate::engine::{enclosing_function_name, extract_call_args, iss};
use crate::rules::*;
use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

static RE_ELSE_PAREN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\belse\s*\(").unwrap());
static RE_STR_COMPARE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[=!]=\s*""#).unwrap());
static RE_TASK_INT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bset_task\s*\(\s*(\d+)\s*,").unwrap());
static RE_PEV_FLOAT_INT: LazyLock<Regex> = LazyLock::new(|| {
    // literal 0 is exempt: its bit pattern equals 0.0
    Regex::new(r"\bset_pev\s*\(\s*[^,]+,\s*pev_(health|gravity|maxspeed|speed|dmg|takedamage|animtime|framerate|scale|renderamt|frame|fuser[1-4])\s*,\s*-?0*[1-9]\d*\s*\)").unwrap()
});
static RE_CS_INT_FLOAT: LazyLock<Regex> = LazyLock::new(|| {
    // cstrike.inc single-arg Float: setters; literal 0 is exempt (bit-identical to 0.0)
    Regex::new(r"\bcs_set_(?:user_lastactivity|hostage_lastuse|hostage_nextuse|c4_explode_time)\s*\(\s*[^,]+,\s*-?0*[1-9]\d*\s*\)").unwrap()
});
static RE_FUN_INT_FLOAT: LazyLock<Regex> = LazyLock::new(|| {
    // fun.inc: set_user_maxspeed(index, Float:speed)/set_user_gravity(index, Float:gravity).
    // Heavily used in ZP class scripts; literal 0 is exempt (bit-identical to 0.0).
    Regex::new(r"\bset_user_(?:maxspeed|gravity)\s*\(\s*[^,]+,\s*-?0*[1-9]\d*\s*\)").unwrap()
});
static RE_INT_NATIVE_FLOAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(set_user_health|set_user_armor|set_user_frags|cs_set_user_money|zp_ammopacks_set)\s*\(\s*[^,]+,\s*-?\d+\.\d+").unwrap()
});
#[derive(Clone, Copy, PartialEq)]
enum EfType {
    /// Float scalar (HLSDK `float`) - a bare int literal here is bit-reinterpreted as ~1e-44/1e-43.
    F,
    /// Int/entity/bool/byte/ushort scalar - a float literal here is bit-reinterpreted as a huge int.
    I,
}

/// Positional parameter types for EngFunc_* selectors, taken verbatim from the HLSDK
/// signatures documented in amxmodx's fakemeta_const.inc. Index 0 = first real parameter
/// (i.e. engfunc() call arg index 1, right after the EngFunc_X selector). `None` = vector/
/// string/handle argument - never a bare literal in practice, so left unchecked.
static ENGFUNC_PARAM_TYPES: &[(&str, &[Option<EfType>])] = &[
    ("EngFunc_WalkMove", &[Some(EfType::I), Some(EfType::F), Some(EfType::F), Some(EfType::I)]),
    ("EngFunc_MoveToOrigin", &[Some(EfType::I), None, Some(EfType::F), Some(EfType::I)]),
    ("EngFunc_TraceLine", &[None, None, Some(EfType::I), Some(EfType::I), Some(EfType::I)]),
    ("EngFunc_TraceHull", &[None, None, Some(EfType::I), Some(EfType::I), Some(EfType::I), Some(EfType::I)]),
    ("EngFunc_TraceModel", &[None, None, Some(EfType::I), Some(EfType::I), Some(EfType::I)]),
    ("EngFunc_TraceSphere", &[None, None, Some(EfType::I), Some(EfType::F), Some(EfType::I), Some(EfType::I)]),
    ("EngFunc_TraceMonsterHull", &[Some(EfType::I), None, None, Some(EfType::I), Some(EfType::I), Some(EfType::I)]),
    ("EngFunc_GetAimVector", &[Some(EfType::I), Some(EfType::F), None]),
    ("EngFunc_EmitSound", &[Some(EfType::I), Some(EfType::I), None, Some(EfType::F), Some(EfType::F), Some(EfType::I), Some(EfType::I)]),
    ("EngFunc_EmitAmbientSound", &[Some(EfType::I), None, None, Some(EfType::F), Some(EfType::F), Some(EfType::I), Some(EfType::I)]),
    ("EngFunc_ParticleEffect", &[None, None, Some(EfType::F), Some(EfType::F)]),
    ("EngFunc_SetClientMaxspeed", &[Some(EfType::I), Some(EfType::F)]),
    ("EngFunc_AnimationAutomove", &[Some(EfType::I), Some(EfType::F)]),
    ("EngFunc_CrosshairAngle", &[Some(EfType::I), Some(EfType::F), Some(EfType::F)]),
    ("EngFunc_FadeClientVolume", &[Some(EfType::I), Some(EfType::I), Some(EfType::I), Some(EfType::I), Some(EfType::I)]),
    ("EngFunc_RunPlayerMove", &[Some(EfType::I), None, Some(EfType::F), Some(EfType::F), Some(EfType::F), Some(EfType::I), Some(EfType::I), Some(EfType::I)]),
    ("EngFunc_StaticDecal", &[None, Some(EfType::I), Some(EfType::I), Some(EfType::I)]),
    ("EngFunc_BuildSoundMsg", &[Some(EfType::I), Some(EfType::I), None, Some(EfType::F), Some(EfType::F), Some(EfType::I), Some(EfType::I), Some(EfType::I), Some(EfType::I), None, Some(EfType::I)]),
    ("EngFunc_PlaybackEvent", &[Some(EfType::I), Some(EfType::I), Some(EfType::I), Some(EfType::F), None, None, Some(EfType::F), Some(EfType::F), Some(EfType::I), Some(EfType::I), Some(EfType::I), Some(EfType::I)]),
    ("EngFunc_MessageBegin", &[Some(EfType::I), Some(EfType::I), None, Some(EfType::I)]),
    ("EngFunc_WriteCoord", &[Some(EfType::F)]),
    ("EngFunc_WriteAngle", &[Some(EfType::F)]),
    ("EngFunc_WriteByte", &[Some(EfType::I)]),
    ("EngFunc_WriteChar", &[Some(EfType::I)]),
    ("EngFunc_WriteShort", &[Some(EfType::I)]),
    ("EngFunc_WriteLong", &[Some(EfType::I)]),
    ("EngFunc_WriteEntity", &[Some(EfType::I)]),
];
static RE_INT_LITERAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-?\d+$").unwrap());
static RE_FLOAT_LITERAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-?\d+\.\d+$").unwrap());

/// Check every engfunc() call on `line` against ENGFUNC_PARAM_TYPES; returns
/// (rule_id, message) for each positional literal/type mismatch found.
fn engfunc_param_mismatches(line: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for args in extract_call_args(line, "engfunc") {
        if args.is_empty() {
            continue;
        }
        let selector = args[0].trim();
        let Some((_, types)) = ENGFUNC_PARAM_TYPES.iter().find(|(name, _)| *name == selector) else {
            continue;
        };
        for (i, expected) in types.iter().enumerate() {
            let Some(kind) = expected else { continue };
            let Some(arg) = args.get(i + 1) else { continue };
            let arg = arg.trim();
            match kind {
                EfType::I if RE_FLOAT_LITERAL.is_match(arg) => {
                    out.push(("engfunc_int_float", format!(
                        "float literal {} into {}'s int/entity parameter #{} (warning 213); reinterpreted bit-for-bit as a huge integer - drop the decimals",
                        arg, selector, i + 1
                    )));
                }
                // int 0 is bit-identical to float 0.0 - only non-zero int literals are unsafe here
                EfType::F if RE_INT_LITERAL.is_match(arg) && arg.trim_start_matches('-') != "0" => {
                    out.push(("engfunc_float_int", format!(
                        "integer literal {} into {}'s Float parameter #{} (warning 213); reinterpreted bit-for-bit as ~1e-44 - add .0",
                        arg, selector, i + 1
                    )));
                }
                _ => {}
            }
        }
    }
    out
}

/// Positional parameter types for the handful of Ham_* forwards that mix Float and Int/entity
/// scalars AND are actually used in CS/ZP scripting (ham_const.inc documents ~470 forwards
/// total, but most are mod-specific to TFC/NS/SC/ESF/DOD/TS and irrelevant here). Verified
/// against each constant's own "Forward params:" doc comment. Index 0 = `this` (args[1] of the
/// ExecuteHam(B) call); `None` = vector/string argument, left unchecked (see ENGFUNC_PARAM_TYPES).
static HAM_PARAM_TYPES: &[(&str, &[Option<EfType>])] = &[
    ("Ham_TakeDamage", &[Some(EfType::I), Some(EfType::I), Some(EfType::I), Some(EfType::F), Some(EfType::I)]),
    ("Ham_TakeHealth", &[Some(EfType::I), Some(EfType::F), Some(EfType::I)]),
    ("Ham_TraceAttack", &[Some(EfType::I), Some(EfType::I), Some(EfType::F), None, Some(EfType::I), Some(EfType::I)]),
    ("Ham_Use", &[Some(EfType::I), Some(EfType::I), Some(EfType::I), Some(EfType::I), Some(EfType::F)]),
    ("Ham_CS_Player_Blind", &[Some(EfType::I), Some(EfType::F), Some(EfType::F), Some(EfType::F), Some(EfType::I)]),
    ("Ham_CS_Player_GetAutoaimVector", &[Some(EfType::I), Some(EfType::F), None]),
];

/// SetHamParam{Integer,Float}(which, ..) numbers "which" differently from ExecuteHam's own
/// argument list: a Float:vec[3] forward parameter is addressed as 3 CONSECUTIVE Float
/// `which` slots (one per x/y/z component), not 1 slot. Confirmed against real-world usage:
/// `SetHamParamFloat(4/5/6, direction[0/1/2] * resist)` inside a Ham_TraceAttack hook (direction
/// is TraceAttack's 4th forward param) - an earlier version of this table treated the vector as
/// a single unchecked slot and misread components 5/6 as hitting the (nonexistent) int slots
/// that follow, a false positive caught by corpus validation. Only forwards actually reachable
/// via SetHamParam need an entry here (RegisterHam pre-hooks only).
static HAM_WHICH_PARAM_TYPES: &[(&str, &[EfType])] = &[
    ("Ham_TakeDamage", &[EfType::I, EfType::I, EfType::I, EfType::F, EfType::I]),
    ("Ham_TakeHealth", &[EfType::I, EfType::F, EfType::I]),
    ("Ham_TraceAttack", &[EfType::I, EfType::I, EfType::F, EfType::F, EfType::F, EfType::F, EfType::I, EfType::I]),
    ("Ham_Use", &[EfType::I, EfType::I, EfType::I, EfType::I, EfType::F]),
    ("Ham_CS_Player_Blind", &[EfType::I, EfType::F, EfType::F, EfType::F, EfType::I]),
    ("Ham_CS_Player_GetAutoaimVector", &[EfType::I, EfType::F]),
];

fn ham_param_mismatches(line: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for func in ["ExecuteHam", "ExecuteHamB"] {
        for args in extract_call_args(line, func) {
            if args.is_empty() {
                continue;
            }
            let selector = args[0].trim();
            let Some((_, types)) = HAM_PARAM_TYPES.iter().find(|(name, _)| *name == selector) else {
                continue;
            };
            for (i, expected) in types.iter().enumerate() {
                let Some(kind) = expected else { continue };
                let Some(arg) = args.get(i + 1) else { continue };
                let arg = arg.trim();
                match kind {
                    EfType::I if RE_FLOAT_LITERAL.is_match(arg) => {
                        out.push(("ham_int_float", format!(
                            "float literal {} into {}'s int/entity parameter #{} ({}(...)) - reinterpreted bit-for-bit as a huge integer - drop the decimals",
                            arg, selector, i + 1, func
                        )));
                    }
                    // int 0 is bit-identical to float 0.0 - only non-zero int literals are unsafe here
                    EfType::F if RE_INT_LITERAL.is_match(arg) && arg.trim_start_matches('-') != "0" => {
                        out.push(("ham_float_int", format!(
                            "integer literal {} into {}'s Float parameter #{} ({}(...)) - reinterpreted bit-for-bit as ~1e-44/1e-43 - add .0",
                            arg, selector, i + 1, func
                        )));
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

/// Positional parameter types for a handful of amxmodx.inc core natives whose signature mixes
/// Float: and plain-int params (unlike engfunc/ExecuteHam these are NORMALLY tagged natives, so
/// amxxpc already emits warning 213 for a mismatch here - same "catch it before compiling"
/// posture as set_task_int_interval/pev_float_int, just extended to more natives). Index 0 is
/// the FIRST real argument (no selector to skip, unlike ENGFUNC/HAM_PARAM_TYPES).
static AMXMODX_PARAM_TYPES: &[(&str, &[Option<EfType>])] = &[
    ("set_hudmessage", &[Some(EfType::I), Some(EfType::I), Some(EfType::I), Some(EfType::F), Some(EfType::F), Some(EfType::I), Some(EfType::F), Some(EfType::F), Some(EfType::F), Some(EfType::F), Some(EfType::I), Some(EfType::I)]),
    ("set_dhudmessage", &[Some(EfType::I), Some(EfType::I), Some(EfType::I), Some(EfType::F), Some(EfType::F), Some(EfType::I), Some(EfType::F), Some(EfType::F), Some(EfType::F), Some(EfType::F)]),
    ("emit_sound", &[Some(EfType::I), Some(EfType::I), None, Some(EfType::F), Some(EfType::F), Some(EfType::I), Some(EfType::I)]),
    ("change_task", &[Some(EfType::I), Some(EfType::F), Some(EfType::I)]),
];

fn amxmodx_param_mismatches(line: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for (name, types) in AMXMODX_PARAM_TYPES {
        for args in extract_call_args(line, name) {
            for (i, expected) in types.iter().enumerate() {
                let Some(kind) = expected else { continue };
                let Some(arg) = args.get(i) else { continue };
                let arg = arg.trim();
                match kind {
                    EfType::I if RE_FLOAT_LITERAL.is_match(arg) => {
                        out.push(("amxmodx_int_float", format!(
                            "float literal {} into {}()'s int parameter #{} (warning 213) - reinterpreted bit-for-bit as a huge integer - drop the decimals",
                            arg, name, i + 1
                        )));
                    }
                    EfType::F if RE_INT_LITERAL.is_match(arg) && arg.trim_start_matches('-') != "0" => {
                        out.push(("amxmodx_float_int", format!(
                            "integer literal {} into {}()'s Float parameter #{} (warning 213) - reinterpreted bit-for-bit as ~1e-44/1e-43 - add .0",
                            arg, name, i + 1
                        )));
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

/// Engine module (engine.inc) entity_get_*/entity_set_* natives are keyed by an EV_* constant
/// whose prefix names the field's real data family (engine_const.inc). The native family
/// picked by the caller (int/float/vector/edict/string/byte) is never cross-checked by the
/// compiler against that prefix - passing an EV_FL_ (Float) field to entity_set_int(), for
/// example, silently reads/writes the wrong bit pattern.
static ENGINE_EV_PREFIX_FAMILY: &[(&str, &str)] = &[
    ("EV_INT_", "int"),
    ("EV_FL_", "float"),
    ("EV_VEC_", "vector"),
    ("EV_ENT_", "edict"),
    ("EV_SZ_", "string"),
    ("EV_BYTE_", "byte"),
];
static RE_ENTITY_GETSET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bentity_(?:get|set)_(int|float|vector|edict2?|string|byte)\s*\(\s*[^,]+,\s*(EV_[A-Za-z0-9_]+)").unwrap()
});

fn entity_ev_mismatch(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    for caps in RE_ENTITY_GETSET.captures_iter(line) {
        let func_family = match caps.get(1).unwrap().as_str() {
            "edict2" => "edict",
            other => other,
        };
        let constant = caps.get(2).unwrap().as_str();
        if let Some((prefix, expected)) = ENGINE_EV_PREFIX_FAMILY.iter().find(|(p, _)| constant.starts_with(p))
            && *expected != func_family {
            out.push(format!(
                "{} ({}* field) passed to entity_*_{}() - use entity_*_{}() instead, or the wrong bit pattern is read/written",
                constant, prefix, func_family, expected
            ));
        }
    }
    out
}
static RE_USERID_INDEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w+\[\s*get_user_userid\s*\(").unwrap());
static RE_FIND_ENT_CONST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"while\s*\(\s*\(?\s*\w+\s*=\s*(?:find_ent_by_(?:class|owner|target)\s*\(\s*(?:-1|0)\s*,|engfunc\s*\(\s*EngFunc_FindEntityByString\s*,\s*(?:-1|0)\s*,)").unwrap()
});
static RE_PRECACHE_MP3: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\b(precache_sound|emit_sound)\s*\([^)]*"[^"]+\.mp3""#).unwrap()
});
static RE_SOUND_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\b(precache_sound|emit_sound)\s*\([^"]*"sound/"#).unwrap()
});
static RE_MP3_LOADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)client_cmd\s*\([^;]*"(?:mp3\s+play|spk)[^"]*loading"#).unwrap()
});
static RE_TE_RELIABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bmessage_begin\s*\(\s*MSG_(ALL|ONE)\s*,\s*SVC_TEMPENTITY").unwrap()
});
static RE_CHANGELEVEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"server_cmd\s*\(\s*"changelevel"#).unwrap());
static RE_DEPRECATED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(md5|md5_file|strbreak)\s*\(").unwrap());
static RE_GEOIP_CODE_OVERFLOW: LazyLock<Regex> = LazyLock::new(|| {
    // geoip_code2/3 (not the _ex variants) document a one-cell buffer overflow on unknown IPs.
    Regex::new(r"\b(geoip_code[23])\s*\(").unwrap()
});
static RE_DEPRECATED_DISCONNECT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bpublic\s+client_disconnect\s*\(").unwrap()
});
static RE_RESERVED_DEFINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*#define\s+(MAX_PLAYERS|MAX_NAME_LENGTH|MAX_STRING_LENGTH|MAX_MOTD_LENGTH|MAX_IP_LENGTH|MAX_AUTHID_LENGTH)\b").unwrap()
});
static RE_CONST_COND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bif\s*\(\s*(0|1|true|false)\s*\)").unwrap());
static RE_EMPTY_STMT_HEAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(if|while)\s*\(").unwrap());
static RE_SELF_ASSIGN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*([A-Za-z_][\w\[\]]*)\s*=\s*([A-Za-z_][\w\[\]]*)\s*;?\s*$").unwrap()
});
static RE_STRING_ASSIGN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*([A-Za-z_]\w*)\s*=\s*"([^"]*)"\s*;?\s*$"#).unwrap()
});
static RE_ARRAY_SIZE_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bnew\s+(?:const\s+)?(?:\w+:)?(\w+)\s*\[\s*(\d+)\s*\]").unwrap()
});
static RE_ARR_WRITE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([A-Za-z_]\w*)\[\s*(-?\d+)\s*\]\s*=").unwrap());
static RE_ARR_CMP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([A-Za-z_]\w*)\s*(==|!=)\s*([A-Za-z_]\w*)\b").unwrap());
static RE_CMP_STMT: LazyLock<Regex> = LazyLock::new(|| {
    // require the trailing `;` - without it the line is usually a multi-line condition
    Regex::new(r"^\s*[A-Za-z_][\w\[\]]*\s*==\s*[^;=|&<>]+;\s*$").unwrap()
});
static RE_STRLEN_LOOP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"for\s*\([^;]*;[^;]*\bstrlen\s*\(").unwrap());
static RE_GET_CVAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bget_cvar_(num|float|string)\s*\(").unwrap());
static RE_NEW_ARRAY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bnew\s+(?:const\s+)?(?:\w+:)?\w+\s*\[\s*(\d+)\s*\]").unwrap());
static RE_RW_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(read_file|write_file)\s*\(").unwrap());
static RE_PRECACHE_CALL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bprecache_(model|sound|generic)\s*\(").unwrap());
static RE_DIV_RUNTIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[/%]\s*(get_playersnum|get_maxplayers|get_pcvar_num|get_pcvar_float)\s*\(").unwrap()
});
static RE_PRAGMA_DYNAMIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#pragma\s+dynamic").unwrap());
static RE_GLOBAL_NEW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^new\s+(.+)$").unwrap());
static RE_DECL_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|,)\s*(?:const\s+)?(?:\w+:)?([A-Za-z_]\w*)").unwrap());
static RE_LOCAL_NEW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bnew\s+(?:const\s+)?(?:\w+:)?([A-Za-z_]\w*)").unwrap());
static RE_PLAYER_ARR_32: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bnew\s+(?:const\s+)?(?:\w+:)?([A-Za-z_]\w*)\s*\[\s*32\s*\]").unwrap()
});
static RE_LOOP_HEADER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(for|while)\s*\(").unwrap());
static RE_CONTAIN_COND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:if|while)\s*\(\s*!?\s*(contain|containi)\s*\(").unwrap()
});
static RE_STRCMP_COND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:if|while)\s*\(\s*strcmp\s*\(").unwrap());
static RE_SQL_FIELDNUM_COND: LazyLock<Regex> = LazyLock::new(|| {
    // sqlx.inc: SQL_FieldNameToNum returns -1 on failure, but column 0 is a valid, falsy result.
    Regex::new(r"\b(?:if|while)\s*\(\s*!?\s*SQL_FieldNameToNum\s*\(").unwrap()
});
static RE_FUNCID_COND: LazyLock<Regex> = LazyLock::new(|| {
    // amxmodx.inc: get_func_id/get_xvar_id return -1 on failure, but id 0 is a valid, falsy result.
    Regex::new(r"\b(?:if|while)\s*\(\s*!?\s*(?:get_func_id|get_xvar_id)\s*\(").unwrap()
});
static RE_FUNCID_ASSIGN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b([A-Za-z_]\w*)\s*=\s*(?:get_func_id|get_xvar_id)\s*\(").unwrap()
});
static RE_CMP_OP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[=!<>]=|[<>]").unwrap());
static RE_ZP_REG_STMT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*zp_(items|class_zombie|class_human|gamemodes)_register\s*\(").unwrap()
});
static RE_ZP_GET_INIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bzp_(?:items|class_zombie|class_human|gamemodes)_get_(?:id|count)\s*\(").unwrap()
});
static RE_ZP_FW_INFECT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"public\s+zp_fw_core_(?:infect|cure)(?:_post)?\s*\(\s*\w+\s*,\s*(\w+)\s*\)").unwrap()
});
static RE_ZP_SELECT_PRE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"public\s+zp_fw_(?:items|class_zombie|class_human)_select_pre\s*\(\s*\w+\s*,\s*(\w+)").unwrap()
});
static RE_ZP_CORE_PRE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"public\s+zp_fw_core_(?:infect|cure)_pre\s*\(").unwrap()
});
static RE_ZP43_NATIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bzp_(get_user_zombie|register_extra_item|register_zombie_class|get_user_ammo_packs|set_user_ammo_packs)\s*\(").unwrap()
});
static RE_DEATHMSG_REG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"register_event\s*\(\s*"DeathMsg"\s*,\s*"(\w+)""#).unwrap()
});
static RE_READ_DATA1: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"new\s+(\w+)\s*=\s*read_data\s*\(\s*1\s*\)").unwrap());
static RE_PRECACHE_MODEL_LIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"precache_model\s*\(\s*"([^"]+)""#).unwrap());
static RE_SET_MODEL_LIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:entity_set_model\s*\(\s*[^,]+,\s*|EngFunc_SetModel\s*,\s*[^,]+,\s*|set_pev\s*\(\s*[^,]+,\s*pev_model\s*,\s*)"([^"]+)""#).unwrap()
});
static RE_CREATE_ENT_ANY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bcreate_entity\s*\(|EngFunc_CreateNamedEntity").unwrap()
});
static RE_REMOVE_ENT_ANY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"remove_entity\s*\(|REMOVE_ENTITY|EngFunc_RemoveEntity|FL_KILLME").unwrap()
});
static RE_FWD_ZERO_ARG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"public\s+(plugin_init|plugin_cfg|plugin_precache|plugin_end|plugin_natives)\s*\(\s*([^)\s][^)]*)\)").unwrap()
});
static RE_FWD_ONE_ARG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"public\s+(client_putinserver|client_command|client_infochanged)\s*\(([^)]*)\)").unwrap()
});
static RE_CASE_LABEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*case\s+[^:]+:\s*(//.*)?$").unwrap());
static RE_PP_DIRECTIVE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*#(if|ifdef|ifndef|else|elseif|emit|endif)\b").unwrap());
static RE_CALLBACK_STR: LazyLock<Regex> = LazyLock::new(|| {
    // first arg (no comma/paren inside), then the quoted callback name
    Regex::new(r#"\b(register_clcmd|register_concmd|register_srvcmd|register_logevent|register_message|menu_create)\s*\([^,()]+,\s*"([A-Za-z_]\w*)""#).unwrap()
});
static RE_PUBLIC_HANDLED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"return\s+PLUGIN_HANDLED\b").unwrap());
static RE_IDENT_ONLY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_]\w*$").unwrap());
static RE_HAM_DMG_CB: LazyLock<Regex> = LazyLock::new(|| {
    // Ham_TakeDamage only: the bullet/pellet path is the confirmed crash. Ham_Touch is
    // excluded on purpose - self-removing pickups in Touch are the ubiquitous safe idiom.
    Regex::new(r#"RegisterHam\s*\(\s*Ham_(TakeDamage)\s*,\s*"[^"]*"\s*,\s*"(\w+)""#).unwrap()
});
static RE_SYNC_REMOVE_ENT: LazyLock<Regex> = LazyLock::new(|| {
    // Immediate edict free. FL_KILLME is the SAFE deferred pattern - intentionally excluded.
    Regex::new(r"\bremove_entity\s*\(|\bEngFunc_RemoveEntity\b|\bREMOVE_ENTITY\s*\(").unwrap()
});
static RE_REGISTERHAM_ANY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"RegisterHam\s*\(\s*(Ham_\w+)\s*,\s*"[^"]*"\s*,\s*"(\w+)""#).unwrap()
});
static RE_SET_HAM_PARAM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bSetHamParam(Integer|Float)\s*\(\s*(\d+)\s*,").unwrap()
});

/// Strip string/char literal contents and `//` comments. `esc` is the file's
/// escape character: AMXX defaults to `^`, overridable via `#pragma ctrlchar`.
/// Returns the sanitized line and whether a double-quoted string was left open.
pub(crate) fn sanitize_line(line: &str, esc: char) -> (String, bool) {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut in_str = false;
    let mut in_char = false;
    while let Some(c) = chars.next() {
        if in_str {
            if c == esc {
                chars.next();
                continue;
            }
            if c == '"' {
                in_str = false;
                out.push('"');
            }
            continue;
        }
        if in_char {
            if c == esc {
                chars.next();
                continue;
            }
            if c == '\'' {
                in_char = false;
                out.push('\'');
            }
            continue;
        }
        match c {
            '"' => { in_str = true; out.push('"'); }
            '\'' => { in_char = true; out.push('\''); }
            '/' if chars.peek() == Some(&'/') => break,
            _ => out.push(c),
        }
    }
    (out, in_str)
}

/// True if the byte offset `pos` in `line` is outside any string literal.
fn outside_string(line: &str, pos: usize, esc: char) -> bool {
    let mut in_str = false;
    let mut skip = false;
    for (i, c) in line.char_indices() {
        if i >= pos { break; }
        if skip { skip = false; continue; }
        if in_str && c == esc { skip = true; continue; }
        if c == '"' { in_str = !in_str; }
    }
    !in_str
}

/// True when the balanced `(...)` condition starting after the opening paren is
/// immediately followed by only `;` (an empty statement).
fn condition_ends_with_semicolon(after_paren: &str) -> bool {
    let mut depth = 1i32;
    for (i, c) in after_paren.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return after_paren[i + 1..].trim() == ";";
                }
            }
            _ => {}
        }
    }
    false
}

/// Single `=` (not ==/!=/<=/>=/compound) at paren depth 1 of an if/while condition.
/// Depth >= 2 (the `if ((x = y))` idiom and call arguments) is not flagged.
fn assignment_in_condition(line: &str) -> bool {
    let Some(m) = Regex::new(r"\b(if|while)\s*\(").unwrap().find(line) else { return false; };
    let cond = &line[m.end()..];
    let mut depth = 1i32;
    let bytes: Vec<char> = cond.chars().collect();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            '(' => depth += 1,
            ')' => { depth -= 1; if depth == 0 { break; } }
            '=' if depth == 1 => {
                let prev = if i > 0 { bytes[i - 1] } else { ' ' };
                let next = if i + 1 < bytes.len() { bytes[i + 1] } else { ' ' };
                if next != '=' && !"=!<>+-*/%&|^".contains(prev) {
                    return true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

pub fn run(raw_clean: &str, lines: &[&str], config: &RulesConfig, issues: &mut Vec<crate::rules::LintIssue>) {
    let esc = if raw_clean.contains(r"#pragma ctrlchar '\'") { '\\' } else { '^' };
    let sanitized: Vec<(String, bool)> = lines.iter().map(|l| sanitize_line(l, esc)).collect();

    // Shared context: brace depth per line, loop membership, function names.
    let mut depth_before: Vec<i32> = Vec::with_capacity(lines.len());
    let mut in_loop: Vec<bool> = Vec::with_capacity(lines.len());
    {
        let mut depth = 0i32;
        let mut loop_stack: Vec<i32> = Vec::new();
        let mut pending_loop = false;
        for (san, _) in &sanitized {
            depth_before.push(depth);
            while let Some(&top) = loop_stack.last() {
                if depth < top { loop_stack.pop(); } else { break; }
            }
            in_loop.push(!loop_stack.is_empty());
            let opens = san.matches('{').count() as i32;
            let closes = san.matches('}').count() as i32;
            let is_loop_header = RE_LOOP_HEADER.is_match(san);
            if is_loop_header && opens > 0 {
                loop_stack.push(depth + 1);
            } else if is_loop_header {
                pending_loop = true;
            } else if pending_loop {
                if opens > 0 { loop_stack.push(depth + 1); }
                pending_loop = false;
            }
            depth += opens - closes;
        }
    }

    let publics = find_publics(raw_clean);
    let mut function_names: Vec<String> = publics.iter().map(|n| n.to_string()).collect();
    function_names.extend(find_nonpublics(raw_clean, &publics));

    let has_pragma_dynamic = RE_PRAGMA_DYNAMIC.is_match(raw_clean);
    let raw_sq = squash(raw_clean);

    // Declared array sizes (any scope) for string_assign.
    let mut array_sizes: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (san, _) in &sanitized {
        for caps in RE_ARRAY_SIZE_DECL.captures_iter(san) {
            if let Ok(n) = caps.get(2).unwrap().as_str().parse::<usize>() {
                array_sizes.entry(caps.get(1).unwrap().as_str().to_string()).or_insert(n);
            }
        }
    }

    // Globals (depth 0 `new` declarations) for shadowing / player_array_32.
    let mut globals: HashSet<String> = HashSet::new();
    let mut player32: Vec<(usize, String)> = Vec::new();
    for (i, (san, _)) in sanitized.iter().enumerate() {
        if depth_before[i] != 0 { continue; }
        if let Some(caps) = RE_GLOBAL_NEW.captures(san.trim_start()) {
            let decls = caps.get(1).unwrap().as_str();
            let stop = decls.find(&['=', '['][..]).unwrap_or(decls.len());
            for c in RE_DECL_NAME.captures_iter(&decls[..stop.min(decls.len())]) {
                globals.insert(c.get(1).unwrap().as_str().to_string());
            }
            // multi declarations after `[..]` are rare; also record simple names conservatively
            for c in RE_PLAYER_ARR_32.captures_iter(san) {
                player32.push((i + 1, c.get(1).unwrap().as_str().to_string()));
            }
        }
    }

    for (i, line) in lines.iter().enumerate() {
        let lineno = i + 1;
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with("//") || stripped.starts_with('*') { continue; }
        let (san, unterminated) = &sanitized[i];
        let san_trim = san.trim();

        // --- compile-structure ---
        if config.enabled("else_paren") && RE_ELSE_PAREN.is_match(san) && !san.contains("else if") {
            issues.push(iss(lineno, "'else (cond)' derails the parser (error 029/010) - use 'else if (cond)'".into(), "else_paren", false));
        }

        if config.enabled("unterminated_string") && *unterminated
            && !line.trim_end().ends_with('\\') && !line.trim_end().ends_with('^') {
            // a continuation of a multi-line string starts mid-string on this line
            let prev_continues = lines[..i].iter().rev().find(|l| !l.trim().is_empty())
                .map(|l| l.trim_end().ends_with('\\') || l.trim_end().ends_with('^'))
                .unwrap_or(false);
            if !prev_continues {
                issues.push(iss(lineno, "unterminated string literal (error 037: possibly non-terminated string)".into(), "unterminated_string", false));
            }
        }

        if config.enabled("line_too_long") && line.len() > 511 {
            issues.push(iss(lineno, format!("line is {} chars; amxxpc 1.8.x errors past 511 (error 075: input line too long) - split the statement", line.len()), "line_too_long", false));
        }

        if config.enabled("empty_statement") && let Some(m) = RE_EMPTY_STMT_HEAD.find(san)
            && condition_ends_with_semicolon(&san[m.end()..]) {
            // `while (...);` right after `}` is a do-while terminator
            let prev = lines[..i].iter().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim()).unwrap_or("");
            if !(san_trim.starts_with("while") && (prev == "}" || prev.ends_with('}'))) {
                issues.push(iss(lineno, "semicolon right after the condition detaches the block below (error 036: empty statement)".into(), "empty_statement", false));
            }
        }

        if config.enabled("stacked_case") && RE_CASE_LABEL.is_match(san)
            && let Some(next) = lines[i + 1..].iter().find(|l| !l.trim().is_empty())
                && next.trim_start().starts_with("case ") {
                issues.push(iss(lineno, "Pawn switch has no fallthrough; stacked 'case A:' 'case B:' does not compile - use 'case A, B:'".into(), "stacked_case", false));
            }

        // --- correctness ---
        if config.enabled("string_literal_compare")
            && let Some(m) = RE_STR_COMPARE.find(line)
            && outside_string(line, m.start(), esc) && !san_trim.starts_with('#') {
            issues.push(iss(lineno, "strings cannot be compared with ==/!= (error 033) - use equal()/equali()".into(), "string_literal_compare", false));
        }

        if config.enabled("assignment_in_condition") && assignment_in_condition(san) {
            issues.push(iss(lineno, "assignment inside condition (warning 211) - use == or wrap in double parentheses if intended".into(), "assignment_in_condition", false));
        }

        if config.enabled("comparison_as_statement") && RE_CMP_STMT.is_match(san)
            && !san_trim.starts_with("for") && !san_trim.starts_with("if") && !san_trim.starts_with("while") {
            issues.push(iss(lineno, "comparison used as a statement does nothing (warning 215) - did you mean '='?".into(), "comparison_as_statement", false));
        }

        if config.enabled("self_assignment") && let Some(caps) = RE_SELF_ASSIGN.captures(san)
            && squash(caps.get(1).unwrap().as_str()) == squash(caps.get(2).unwrap().as_str()) {
            issues.push(iss(lineno, "variable assigned to itself (warning 226) - probably a typo in the source variable".into(), "self_assignment", false));
        }

        // assigning a literal that fits is legal Pawn; only a too-long literal is error 047
        if config.enabled("string_assign") && let Some(caps) = RE_STRING_ASSIGN.captures(line)
            && !["new ", "static ", "const ", "stock ", "#"].iter().any(|kw| san_trim.starts_with(kw)) {
            let name = caps.get(1).unwrap().as_str();
            let lit = caps.get(2).unwrap().as_str();
            // effective cells: escape sequences (^x) collapse to one cell, plus terminator
            let mut cells = 1usize;
            let mut esc_next = false;
            for c in lit.chars() {
                if esc_next { esc_next = false; continue; }
                if c == esc { esc_next = true; }
                cells += 1;
            }
            if let Some(size) = array_sizes.get(name)
                && cells > *size {
                issues.push(iss(lineno, format!("string of {} cells assigned to {}[{}] does not fit (error 047: array sizes do not match) - use copy()", cells, name, size), "string_assign", false));
            }
        }

        // Restricted to WRITES (`name[N] =`) on non-declaration lines: a multi-var `new a[4],
        // b[32]` statement has no leading `new` before `b[32]`, so a bare access-style regex
        // misreads every later array's own size declaration as an out-of-bounds access on
        // itself (e.g. `new name[32], authid[32]` -> "authid[32]" looks like an access to
        // array_sizes["authid"]=32). Requiring a trailing `=` and excluding declaration/
        // signature lines confines this to the real bug: an assignment through a literal
        // out-of-bounds index.
        if config.enabled("array_index_oob")
            && !["new ", "static ", "const ", "stock ", "#"].iter().any(|kw| san_trim.starts_with(kw)) {
            for caps in RE_ARR_WRITE.captures_iter(san) {
                let m = caps.get(0).unwrap();
                if san.as_bytes().get(m.end()).copied() == Some(b'=') { continue; } // `==`, not assignment
                let name = caps.get(1).unwrap().as_str();
                let Some(&size) = array_sizes.get(name) else { continue };
                let Ok(idx) = caps.get(2).unwrap().as_str().parse::<i64>() else { continue };
                if idx < 0 || idx as u64 >= size as u64 {
                    issues.push(iss(lineno, format!(
                        "literal index {} is out of bounds for {}[{}] (valid range 0-{}) - amxxpc rejects constant out-of-bounds indices at compile time",
                        idx, name, size, size.saturating_sub(1)
                    ), "array_index_oob", false));
                }
            }
        }

        if config.enabled("array_compare_by_ref") && !san_trim.starts_with('#') {
            for caps in RE_ARR_CMP.captures_iter(san) {
                let a = caps.get(1).unwrap();
                let op = caps.get(2).unwrap().as_str();
                let b = caps.get(3).unwrap();
                if san[b.end()..].starts_with('[') { continue; }
                let (aname, bname) = (a.as_str(), b.as_str());
                if aname != bname && array_sizes.contains_key(aname) && array_sizes.contains_key(bname) {
                    issues.push(iss(lineno, format!(
                        "'{} {} {}' compares two arrays by reference, not by content; Pawn has no array ==/!= - compare element-by-element or use equal()/equali() for strings",
                        aname, op, bname
                    ), "array_compare_by_ref", false));
                }
            }
        }

        if config.enabled("constant_condition") && RE_CONST_COND.is_match(san) {
            issues.push(iss(lineno, "constant condition dead-codes this branch (warnings 205/206) - debugging leftover?".into(), "constant_condition", false));
        }

        if config.enabled("contain_truthy") && let Some(m) = RE_CONTAIN_COND.find(san) {
            let rest = &san[m.end()..];
            let close = rest.find(')').map(|p| &rest[p..]).unwrap_or("");
            if !RE_CMP_OP.is_match(close) {
                issues.push(iss(lineno, "contain()/containi() return -1 when NOT found; bare truthiness inverts the logic - compare with != -1".into(), "contain_truthy", false));
            }
        }

        if config.enabled("strcmp_truthy") && let Some(m) = RE_STRCMP_COND.find(san) {
            let rest = &san[m.end()..];
            let close = rest.find(')').map(|p| &rest[p..]).unwrap_or("");
            if !RE_CMP_OP.is_match(close) && !san.contains("!strcmp") {
                issues.push(iss(lineno, "strcmp() returns 0 on match; bare 'if (strcmp(..))' means strings DIFFER - use equal() or '== 0'".into(), "strcmp_truthy", false));
            }
        }

        if config.enabled("sql_fieldname_truthy") && let Some(m) = RE_SQL_FIELDNUM_COND.find(san) {
            let rest = &san[m.end()..];
            let close = rest.find(')').map(|p| &rest[p..]).unwrap_or("");
            if !RE_CMP_OP.is_match(close) {
                issues.push(iss(lineno, "SQL_FieldNameToNum() returns -1 when the column doesn't exist, but column 0 is a valid (falsy) result; bare 'if (SQL_FieldNameToNum(..))' misreads column 0 as failure - compare with != -1".into(), "sql_fieldname_truthy", false));
            }
        }

        if config.enabled("func_id_truthy") && let Some(m) = RE_FUNCID_COND.find(san) {
            let rest = &san[m.end()..];
            let close = rest.find(')').map(|p| &rest[p..]).unwrap_or("");
            if !RE_CMP_OP.is_match(close) {
                issues.push(iss(lineno, "get_func_id()/get_xvar_id() return -1 on failure, but id 0 is a valid (falsy) result; bare 'if (get_func_id(..))' misreads id 0 as failure - compare with != -1".into(), "func_id_truthy", false));
            }
        }

        // the more common real-world idiom assigns the id to a variable first, then tests it
        // bare a line or two later (e.g. `new x = get_xvar_id(name); if (x) ...`)
        if config.enabled("func_id_truthy") && let Some(caps) = RE_FUNCID_ASSIGN.captures(san) {
            let var = caps.get(1).unwrap().as_str();
            let body_sq = squash(&enclosing_body(lines, i));
            if body_sq.contains(&format!("if({})", var)) || body_sq.contains(&format!("if(!{})", var))
                || body_sq.contains(&format!("while({})", var)) || body_sq.contains(&format!("while(!{})", var)) {
                issues.push(iss(lineno, format!("'{}' holds a get_func_id()/get_xvar_id() result (-1 on failure, but id 0 is valid); a bare 'if ({})' in this function misreads id 0 as failure - compare with != -1", var, var), "func_id_truthy", false));
            }
        }

        if config.enabled("formatex_self") {
            for args in extract_call_args(line, "formatex") {
                if args.len() >= 3 && RE_IDENT_ONLY.is_match(args[0].trim()) {
                    let buf = args[0].trim();
                    let re_word = Regex::new(&format!(r"\b{}\b", regex::escape(buf))).unwrap();
                    if args.iter().skip(2).any(|a| re_word.is_match(a)) {
                        issues.push(iss(lineno, format!("formatex() output buffer \"{}\" is also an input; formatex skips copy-back checking - use format()", buf), "formatex_self", false));
                    }
                }
            }
        }

        // --- tag mismatch ---
        if config.enabled("set_task_int_interval") && let Some(caps) = RE_TASK_INT.captures(san)
            && caps.get(1).unwrap().as_str() != "0" {
            issues.push(iss(lineno, format!("set_task interval {} is an integer (warning 213); the bit pattern becomes ~1e-44s (runs every frame) - write {}.0", caps.get(1).unwrap().as_str(), caps.get(1).unwrap().as_str()), "set_task_int_interval", false));
        }

        if config.enabled("pev_float_int") && RE_PEV_FLOAT_INT.is_match(san) {
            issues.push(iss(lineno, "integer literal into a Float pev field (warning 213); e.g. pev_health 100 becomes ~1.4e-43 (instant death) - add .0".into(), "pev_float_int", false));
        }

        if config.enabled("int_native_float") && RE_INT_NATIVE_FLOAT.is_match(san) {
            issues.push(iss(lineno, "float literal into an integer native (warning 213); 100.0 becomes 1120403456 - drop the decimals or use floatround()".into(), "int_native_float", false));
        }

        if config.enabled("cs_float_int") && RE_CS_INT_FLOAT.is_match(san) {
            issues.push(iss(lineno, "integer literal into a cstrike.inc Float parameter (warning 213); e.g. cs_set_c4_explode_time(id, 10) becomes ~1.4e-44 - add .0".into(), "cs_float_int", false));
        }

        if config.enabled("fun_float_int") && RE_FUN_INT_FLOAT.is_match(san) {
            issues.push(iss(lineno, "integer literal into set_user_maxspeed/set_user_gravity's Float parameter (warning 213); e.g. set_user_gravity(id, 1) becomes ~1.4e-45 - add .0".into(), "fun_float_int", false));
        }

        for (rule_id, msg) in engfunc_param_mismatches(san) {
            if config.enabled(rule_id) {
                issues.push(iss(lineno, msg, rule_id, false));
            }
        }

        if config.enabled("entity_ev_type_mismatch") {
            for msg in entity_ev_mismatch(san) {
                issues.push(iss(lineno, msg, "entity_ev_type_mismatch", false));
            }
        }

        for (rule_id, msg) in ham_param_mismatches(san) {
            if config.enabled(rule_id) {
                issues.push(iss(lineno, msg, rule_id, false));
            }
        }

        for (rule_id, msg) in amxmodx_param_mismatches(san) {
            if config.enabled(rule_id) {
                issues.push(iss(lineno, msg, rule_id, false));
            }
        }

        // --- runtime crashes ---
        if config.enabled("userid_as_index") && RE_USERID_INDEX.is_match(san) {
            issues.push(iss(lineno, "get_user_userid() is a session counter (can be 500+), not a client index - indexing an array with it is run time error 4".into(), "userid_as_index", false));
        }

        if config.enabled("find_ent_no_advance") && RE_FIND_ENT_CONST.is_match(san) {
            issues.push(iss(lineno, "entity-search loop restarts from a constant every iteration - infinite loop, server freezes; pass the previous entity as start index".into(), "find_ent_no_advance", false));
        }

        if config.enabled("div_by_runtime") && let Some(m) = RE_DIV_RUNTIME.find(san)
            && outside_string(line, m.start(), esc) {
            issues.push(iss(lineno, "division/modulo by a runtime value that can be zero (empty server / cvar 0) - run time error 11; guard > 0 first".into(), "div_by_runtime", false));
        }

        if config.enabled("pragma_dynamic_stack") && !has_pragma_dynamic
            && (line.starts_with(' ') || line.starts_with('\t'))
            && let Some(caps) = RE_NEW_ARRAY.captures(san)
            && caps.get(1).unwrap().as_str().parse::<u32>().unwrap_or(0) >= 2048 {
            issues.push(iss(lineno, format!("local array of {} cells can blow the default 4096-cell AMX stack (run time error 3) - add #pragma dynamic or make it global/static", caps.get(1).unwrap().as_str()), "pragma_dynamic_stack", false));
        }

        // --- engine/HLDS ---
        if config.enabled("precache_mp3") && RE_PRECACHE_MP3.is_match(line) {
            issues.push(iss(lineno, ".mp3 cannot go through precache_sound/emit_sound - use precache_generic(\"sound/...\") + client_cmd \"mp3 play\"".into(), "precache_mp3", false));
        }

        if config.enabled("sound_prefix") && RE_SOUND_PREFIX.is_match(line) {
            issues.push(iss(lineno, "precache_sound/emit_sound paths are relative to sound/ - \"sound/x.wav\" resolves to sound/sound/x.wav and never plays".into(), "sound_prefix", false));
        }

        if config.enabled("mp3_loading_path") && RE_MP3_LOADING.is_match(line) {
            issues.push(iss(lineno, "GoldSrc clients reject stufftext containing 'loading' - this mp3/spk path is silently blocked (amxmodx issue #818)".into(), "mp3_loading_path", false));
        }

        if config.enabled("te_reliable") && RE_TE_RELIABLE.is_match(san) {
            issues.push(iss(lineno, "SVC_TEMPENTITY on the reliable channel (MSG_ALL/MSG_ONE) can overflow netchan and kick players - use MSG_BROADCAST/MSG_ONE_UNRELIABLE".into(), "te_reliable", false));
        }

        if config.enabled("changelevel_cmd") && RE_CHANGELEVEL.is_match(line) {
            issues.push(iss(lineno, "server_cmd(\"changelevel\") skips the server_changelevel forward and map validity check - use is_map_valid() + change_level()".into(), "changelevel_cmd", false));
        }

        if config.enabled("hud_channel_range") {
            for args in extract_call_args(san, "set_hudmessage") {
                if let Some(ch) = args.get(10).and_then(|a| a.trim().parse::<i32>().ok())
                    && !(-1..=4).contains(&ch) {
                    issues.push(iss(lineno, format!("set_hudmessage channel {} - clients only have channels 1-4 (or -1 auto); other values are masked by the engine and stomp channels unpredictably", ch), "hud_channel_range", false));
                }
            }
        }

        if config.enabled("geoip_code_overflow") && let Some(caps) = RE_GEOIP_CODE_OVERFLOW.captures(san) {
            let name = caps.get(1).unwrap().as_str();
            issues.push(iss(lineno, format!("{}() overflows its result buffer by one cell on an unknown IP - use {}_ex() instead", name, name), "geoip_code_overflow", false));
        }

        // --- deprecated / defines ---
        if config.enabled("deprecated_symbols") {
            if let Some(caps) = RE_DEPRECATED.captures(san) {
                issues.push(iss(lineno, format!("{}() is deprecated in AMXX 1.9 (warning 233) - use the hasher API / argbreak()", caps.get(1).unwrap().as_str()), "deprecated_symbols", false));
            }
            if RE_DEPRECATED_DISCONNECT.is_match(san) {
                issues.push(iss(lineno, "client_disconnect is deprecated in AMXX 1.9 - client_disconnected also fires for aborted connections (prevents state leaks)".into(), "deprecated_symbols", false));
            }
        }

        if config.enabled("define_reserved_const") && let Some(caps) = RE_RESERVED_DEFINE.captures(line) {
            issues.push(iss(lineno, format!("#define {} redefines an amxconst.inc constant (warning 201); a different value silently desynchronizes buffer sizes", caps.get(1).unwrap().as_str()), "define_reserved_const", false));
        }

        // --- perf (loop / hot path) ---
        if config.enabled("strlen_in_loop") && RE_STRLEN_LOOP.is_match(san) {
            issues.push(iss(lineno, "strlen() in the loop condition is recomputed every iteration (O(n^2)) - cache the length before the loop".into(), "strlen_in_loop", false));
        }

        if in_loop[i] {
            if config.enabled("buffer_in_loop") && let Some(caps) = RE_NEW_ARRAY.captures(san)
                && caps.get(1).unwrap().as_str().parse::<u32>().unwrap_or(0) >= 64 {
                issues.push(iss(lineno, "array declared inside a loop body is re-zeroed every iteration - hoist it out of the loop".into(), "buffer_in_loop", false));
            }
            if config.enabled("read_file_loop") && RE_RW_FILE.is_match(san) {
                issues.push(iss(lineno, "read_file/write_file reopen and rescan the file per call (O(n^2) in loops) - use fopen/fgets/fputs/fclose".into(), "read_file_loop", false));
            }
            if config.enabled("precache_in_loop") && RE_PRECACHE_CALL.is_match(san) {
                issues.push(iss(lineno, "precache_* inside a loop risks the 512-entry engine precache limit (fatal Host_Error at map start)".into(), "precache_in_loop", false));
            }
        }

        if config.enabled("get_cvar_hotpath") && let Some(m) = RE_GET_CVAR.find(san)
            && outside_string(line, m.start(), esc) {
            let f = enclosing_function_name(lines, i, &function_names);
            if !matches!(f.as_deref(), Some("plugin_init") | Some("plugin_cfg") | Some("plugin_precache") | Some("plugin_natives") | Some("plugin_end") | None) {
                issues.push(iss(lineno, "get_cvar_* does a string lookup per call - cache the pointer from register_cvar() and use get_pcvar_* (docs: 'dozens of times faster')".into(), "get_cvar_hotpath", false));
            }
        }

        // --- format injection ---
        if config.enabled("format_injection") {
            let candidates: [(&str, usize); 3] = [("client_print", 3), ("console_print", 2), ("log_amx", 1)];
            for (native, fmt_count) in candidates {
                for args in extract_call_args(san, native) {
                    if args.len() == fmt_count && RE_IDENT_ONLY.is_match(args[fmt_count - 1].trim()) {
                        let ident = args[fmt_count - 1].trim().to_string();
                        let body_sq = squash(&enclosing_body(lines, i));
                        let user_controlled = body_sq.contains(&format!("read_args({},", ident))
                            || (body_sq.contains("read_argv(") && body_sq.contains(&format!(",{},", ident)))
                            || body_sq.contains(&format!("get_user_name({},", ident).replacen(ident.as_str(), "", 1))
                            || Regex::new(&format!(r"get_user_name\([^,]+,{}\b", regex::escape(&ident))).unwrap().is_match(&body_sq);
                        if user_controlled {
                            issues.push(iss(lineno, format!("{}() format argument \"{}\" holds user text; a '%' in chat/nickname is interpreted as a format specifier - use a literal \"%s\"", native, ident), "format_injection", false));
                        }
                    }
                }
            }
        }

        // --- global shadowing ---
        if config.enabled("global_shadowing") && depth_before[i] > 0
            && let Some(caps) = RE_LOCAL_NEW.captures(san) {
            let name = caps.get(1).unwrap().as_str();
            if globals.contains(name) {
                issues.push(iss(lineno, format!("local 'new {}' shadows the global (warning 219) - writes never reach the global", name), "global_shadowing", false));
            }
        }

        // --- ZP50 ---
        if config.enabled("zp50_register_return") && RE_ZP_REG_STMT.is_match(san) {
            issues.push(iss(lineno, "registration id discarded; it is the only handle to filter select_pre/forwards for YOUR item/class - assign it to a global".into(), "zp50_register_return", false));
        }

        if config.enabled("zp50_get_in_init") && RE_ZP_GET_INIT.is_match(san)
            && enclosing_function_name(lines, i, &function_names).as_deref() == Some("plugin_init") {
            issues.push(iss(lineno, "zp50 query natives in plugin_init hit 'Invalid Array Handle' when plugin load order puts you before the core - query in plugin_cfg or forwards".into(), "zp50_get_in_init", false));
        }

        if config.enabled("zp_fw_attacker_guard") && let Some(caps) = RE_ZP_FW_INFECT.captures(san) {
            let attacker = caps.get(1).unwrap().as_str().to_string();
            let body = enclosing_body(lines, i);
            let body_sq = squash(&body);
            let uses = uses_player_native_on(&body, &attacker)
                || body_sq.contains(&format!("zp_ammopacks_set({}", attacker))
                || body_sq.contains(&format!("zp_ammopacks_get({}", attacker))
                || body_sq.contains(&format!("get_user_name({},", attacker));
            let guarded = has_guard(&body, &attacker)
                || body_sq.contains(&format!("!{}", attacker))
                || body_sq.contains(&format!("if({})", attacker))
                || body_sq.contains(&format!("if({}&&", attacker));
            if uses && !guarded {
                issues.push(iss(lineno, format!("zp_fw_core_infect/cure '{}' is 0 for gamemode/admin/console infections (documented) - guard before player natives or it errors every round start", attacker), "zp_fw_attacker_guard", false));
            }
        }

        if config.enabled("zp_select_pre_filter") && let Some(caps) = RE_ZP_SELECT_PRE.captures(san) {
            let param = caps.get(1).unwrap().as_str();
            let body = enclosing_body(lines, i);
            let body_sq = squash(&body);
            let restrictive = body_sq.contains("ZP_ITEM_NOT_AVAILABLE") || body_sq.contains("ZP_ITEM_DONT_SHOW")
                || body_sq.contains("ZP_CLASS_NOT_AVAILABLE") || body_sq.contains("ZP_CLASS_DONT_SHOW");
            // referencing the param at all (cost lookup, comparison, ...) counts as filtering;
            // the bug is ignoring it entirely (manager plugins legitimately apply to all items)
            let refs = Regex::new(&format!(r"\b{}\b", regex::escape(param)))
                .map(|re| re.find_iter(&body).count()).unwrap_or(2);
            if restrictive && refs < 2 {
                issues.push(iss(lineno, format!("select_pre returns a restrictive ZP_* without ever using '{}' - the max across plugins wins, this blocks/hides EVERY item/class server-wide", param), "zp_select_pre_filter", false));
            }
        }

        if config.enabled("zp_select_pre_return") {
            if RE_ZP_SELECT_PRE.is_match(san) {
                let body_sq = squash(&enclosing_body(lines, i));
                // PLUGIN_CONTINUE (=0) aliases ZP_*_AVAILABLE and is harmless
                if body_sq.contains("returnPLUGIN_HANDLED") {
                    issues.push(iss(lineno, "select_pre forwards use ZP_ITEM_*/ZP_CLASS_* return constants; PLUGIN_HANDLED (=1) accidentally means NOT_AVAILABLE".into(), "zp_select_pre_return", false));
                }
            }
            if RE_ZP_CORE_PRE.is_match(san) {
                let body_sq = squash(&enclosing_body(lines, i));
                if body_sq.contains("returnZP_ITEM_") || body_sq.contains("returnZP_CLASS_") {
                    issues.push(iss(lineno, "zp_fw_core_*_pre is blocked with PLUGIN_HANDLED, not ZP_ITEM_*/ZP_CLASS_* constants".into(), "zp_select_pre_return", false));
                }
            }
        }

        // --- forward contracts ---
        if config.enabled("client_command_handled") && RE_PUBLIC_HANDLED.is_match(san)
            && enclosing_function_name(lines, i, &function_names).as_deref() == Some("client_command") {
            issues.push(iss(lineno, "PLUGIN_HANDLED in client_command also starves other plugins' handlers - use PLUGIN_HANDLED_MAIN (amxconst.inc documents this exact case)".into(), "client_command_handled", false));
        }

        if config.enabled("client_connect_actions") && enclosing_function_name(lines, i, &function_names).as_deref() == Some("client_connect") {
            for nat in ["client_print(", "show_menu(", "set_user_", "cs_set_user_", "give_item("] {
                if san.contains(nat) {
                    issues.push(iss(lineno, "client_connect is 'too early to do anything that directly affects the client' (official docs) - move to client_putinserver".into(), "client_connect_actions", false));
                    break;
                }
            }
        }

        // --- unreachable code ---
        if config.enabled("unreachable_code") && !san.contains('}')
            && (san_trim == "return" || (san_trim.starts_with("return") && san_trim[6..].starts_with([' ', ';', '\t']))) {
            let prev = lines[..i].iter().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim()).unwrap_or("");
            let prev_is_branch = (prev.starts_with("if") || prev.starts_with("else") || prev.starts_with("for") || prev.starts_with("while") || prev.starts_with("case") || prev.starts_with("default")) && !prev.contains('{');
            if !prev_is_branch
                && let Some(next) = lines[i + 1..].iter().find(|l| !l.trim().is_empty()) {
                let nt = next.trim();
                // a top-level declaration after the return means the return was a
                // braceless function body, not dead code
                let top_level_decl = ["public ", "stock ", "static ", "forward ", "native ", "new ", "enum"]
                    .iter().any(|kw| nt.starts_with(kw));
                if !nt.starts_with('}') && !nt.starts_with("case") && !nt.starts_with("default")
                    && !nt.starts_with('#') && !nt.starts_with("//") && !nt.starts_with("else")
                    && !nt.starts_with('*') && !nt.starts_with('{') && !top_level_decl {
                    issues.push(iss(lineno, "code after an unconditional return never runs (warning 225)".into(), "unreachable_code", false));
                }
            }
        }
    }

    // ---------- file-level post-passes ----------

    if config.enabled("unbalanced_preprocessor") {
        let mut stack: Vec<usize> = Vec::new();
        let mut else_seen: Vec<bool> = Vec::new();
        for (i, (san, _)) in sanitized.iter().enumerate() {
            let Some(caps) = RE_PP_DIRECTIVE.captures(san) else { continue };
            match caps.get(1).unwrap().as_str() {
                "if" | "ifdef" | "ifndef" => { stack.push(i + 1); else_seen.push(false); }
                "else" => {
                    if stack.is_empty() {
                        issues.push(iss(i + 1, "#else without an open #if (error 026)".into(), "unbalanced_preprocessor", false));
                    } else if let Some(seen) = else_seen.last_mut() {
                        if *seen {
                            issues.push(iss(i + 1, "multiple #else in one #if block (error 060)".into(), "unbalanced_preprocessor", false));
                        }
                        *seen = true;
                    }
                }
                "elseif" => {
                    if stack.is_empty() {
                        issues.push(iss(i + 1, "#elseif without an open #if (error 026)".into(), "unbalanced_preprocessor", false));
                    } else if else_seen.last() == Some(&true) {
                        issues.push(iss(i + 1, "#elseif after #else (error 061)".into(), "unbalanced_preprocessor", false));
                    }
                }
                "endif" => {
                    if stack.pop().is_none() {
                        issues.push(iss(i + 1, "#endif without an open #if (error 026)".into(), "unbalanced_preprocessor", false));
                    }
                    else_seen.pop();
                }
                _ => {}
            }
        }
        for open_line in stack {
            issues.push(iss(open_line, "#if opened here is never closed with #endif".into(), "unbalanced_preprocessor", false));
        }
    }

    // Brace balance: skip when the file uses #else (branches may intentionally unbalance).
    // Only report when the file ends unbalanced - a transient negative that recovers by
    // EOF means our line model missed something, not that the code is broken.
    if config.enabled("unbalanced_braces") && !sanitized.iter().any(|(s, _)| s.trim_start().starts_with("#else")) {
        let mut depth = 0i32;
        let mut last_open = 0usize;
        let mut first_negative = 0usize;
        for (i, (san, _)) in sanitized.iter().enumerate() {
            for c in san.chars() {
                match c {
                    '{' => { depth += 1; last_open = i + 1; }
                    '}' => {
                        depth -= 1;
                        if depth < 0 && first_negative == 0 { first_negative = i + 1; }
                    }
                    _ => {}
                }
            }
        }
        if depth < 0 {
            issues.push(iss(first_negative.max(1), "unmatched closing brace (error 054); every function below this line will also fail (error 010/004)".into(), "unbalanced_braces", false));
        } else if depth > 0 {
            issues.push(iss(last_open, format!("{} unclosed brace(s) at end of file (error 030: compound statement not closed)", depth), "unbalanced_braces", false));
        }
    }

    if config.enabled("forward_arity") {
        for caps in RE_FWD_ZERO_ARG.captures_iter(raw_clean) {
            let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
            issues.push(iss(lineno, format!("{}() takes no parameters (error 025: heading differs from prototype)", caps.get(1).unwrap().as_str()), "forward_arity", false));
        }
        for caps in RE_FWD_ONE_ARG.captures_iter(raw_clean) {
            let params = caps.get(2).unwrap().as_str();
            let count = if params.trim().is_empty() { 0 } else { params.split(',').count() };
            if count != 1 {
                let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
                issues.push(iss(lineno, format!("{}(id) takes exactly 1 parameter, found {} (error 025)", caps.get(1).unwrap().as_str(), count), "forward_arity", false));
            }
        }
    }

    if config.enabled("player_array_32") {
        for (decl_line, name) in &player32 {
            let re_use = Regex::new(&format!(r"\b{}\[\s*(?:id|player)\s*\]", regex::escape(name))).unwrap();
            if re_use.is_match(raw_clean) {
                issues.push(iss(*decl_line, format!("'{}[32]' is indexed by a player id (1..32) - slot 32 overflows on a full server (run time error 4); declare [33] / [MAX_PLAYERS + 1]", name), "player_array_32", false));
            }
        }
    }

    if config.enabled("model_not_precached") {
        let precached: HashSet<&str> = RE_PRECACHE_MODEL_LIT.captures_iter(raw_clean)
            .map(|c| c.get(1).unwrap().as_str()).collect();
        static RE_STOCK_MODEL: LazyLock<Regex> = LazyLock::new(|| {
            // standard game content (w_/v_/p_ weapon models at models/ root) is
            // precached by the engine itself
            Regex::new(r"^models/[wvp]_\w+\.mdl$").unwrap()
        });
        for caps in RE_SET_MODEL_LIT.captures_iter(raw_clean) {
            let model = caps.get(1).unwrap().as_str();
            if model.ends_with(".mdl") && !precached.contains(model) && !RE_STOCK_MODEL.is_match(model) {
                let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
                issues.push(iss(lineno, format!("model \"{}\" is set but never precached in this file - fatal 'SV_ModelIndex: model not precached' if no other plugin precaches it", model), "model_not_precached", false));
            }
        }
    }

    if config.enabled("entity_leak") && RE_CREATE_ENT_ANY.is_match(raw_clean) && !RE_REMOVE_ENT_ANY.is_match(raw_clean)
        && let Some(m) = RE_CREATE_ENT_ANY.find(raw_clean) {
            let lineno = raw_clean[..m.start()].matches('\n').count() + 1;
            issues.push(iss(lineno, "entities are created but never removed anywhere in this file - edicts accumulate until fatal 'ED_Alloc: no free edicts'".into(), "entity_leak", false));
        }

    if config.enabled("callback_not_defined") {
        for caps in RE_CALLBACK_STR.captures_iter(raw_clean) {
            let cb = caps.get(2).unwrap().as_str();
            if function_names.iter().any(|f| f == cb) { continue; }
            // RegisterHam/register_event string args can be event/class names, not callbacks;
            // only flag identifiers that look like function names and are truly absent.
            if !raw_sq.contains(&format!("{}(", cb)) {
                let native = caps.get(1).unwrap().as_str();
                let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
                issues.push(iss(lineno, format!("{} callback \"{}\" has no function definition in this file - plugin fails at load with 'function not found'", native, cb), "callback_not_defined", false));
            }
        }
    }

    if config.enabled("deathmsg_killer_guard") {
        for caps in RE_DEATHMSG_REG.captures_iter(raw_clean) {
            let cb = caps.get(1).unwrap().as_str();
            let body = find_function_body_in(lines, cb);
            if body.is_empty() { continue; }
            if let Some(vcaps) = RE_READ_DATA1.captures(&body) {
                let var = vcaps.get(1).unwrap().as_str().to_string();
                let body_sq = squash(&body);
                let used = body_sq.contains(&format!("[{}]", var))
                    || body_sq.contains(&format!("get_user_name({},", var))
                    || uses_player_native_on(&body, &var);
                let guarded = has_guard(&body, &var)
                    || body_sq.contains(&format!("!{}", var))
                    || body_sq.contains(&format!("if({})", var))
                    || body_sq.contains(&format!("if({}&&", var));
                if used && !guarded {
                    let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
                    issues.push(iss(lineno, format!("DeathMsg killer '{}' (read_data(1)) is 0 for fall/acid/world deaths - guard before using it as index/player", var), "deathmsg_killer_guard", false));
                }
            }
        }
    }

    if config.enabled("remove_entity_in_damage_hook") {
        for caps in RE_HAM_DMG_CB.captures_iter(raw_clean) {
            let cb = caps.get(2).unwrap().as_str();
            let body = find_function_body_in(lines, cb);
            if body.is_empty() { continue; }
            // The safe fix moves remove_entity() into a separate set_task callback, so a
            // hook body that still calls it directly is the crash-prone synchronous case.
            if RE_SYNC_REMOVE_ENT.is_match(&body) {
                let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
                issues.push(iss(lineno, format!("remove_entity() runs synchronously inside the Ham_TakeDamage callback \"{}\" - multi-pellet weapons (shotgun) free the edict mid-FireBullets and later pellets deref it -> server freeze; defer via set_task or set pev_flags FL_KILLME", cb), "remove_entity_in_damage_hook", false));
            }
        }
    }

    if config.enabled("set_ham_param_mismatch") {
        // SetHamParamFloat/Integer(which, ..) picks the setter by hand instead of by tag -
        // "which" is the 1-indexed position in the Ham_* forward's own declared parameter
        // list (e.g. Ham_TakeDamage's function(this, idinflictor, idattacker, Float:damage,
        // damagebits) makes SetHamParamFloat(4, ..) correct and SetHamParamInteger(4, ..)
        // wrong), and nothing enforces that the setter family matches the slot's real type.
        for caps in RE_REGISTERHAM_ANY.captures_iter(raw_clean) {
            let ham = caps.get(1).unwrap().as_str();
            let cb = caps.get(2).unwrap().as_str();
            let Some((_, types)) = HAM_WHICH_PARAM_TYPES.iter().find(|(name, _)| *name == ham) else { continue };
            let body = find_function_body_in(lines, cb);
            if body.is_empty() { continue; }
            let cb_def_re = Regex::new(&format!(r"\bpublic\s+{}\s*\(", regex::escape(cb))).unwrap();
            let cb_start = lines.iter().position(|ln| cb_def_re.is_match(ln));
            for pcaps in RE_SET_HAM_PARAM.captures_iter(&body) {
                let setter_family = if &pcaps[1] == "Float" { EfType::F } else { EfType::I };
                let Ok(which) = pcaps[2].parse::<usize>() else { continue };
                let Some(expected) = which.checked_sub(1).and_then(|i| types.get(i)) else { continue };
                if *expected != setter_family {
                    let lineno = cb_start.map(|s| s + 1 + body[..pcaps.get(0).unwrap().start()].matches('\n').count()).unwrap_or(0);
                    issues.push(iss(lineno, format!(
                        "SetHamParam{}({}, ..) in \"{}\" targets {}'s parameter #{}, which is {} - use SetHamParam{}() instead",
                        &pcaps[1], which, cb, ham, which,
                        if *expected == EfType::F { "Float" } else { "int/entity" },
                        if *expected == EfType::F { "Float" } else { "Integer" }
                    ), "set_ham_param_mismatch", false));
                }
            }
        }
    }

    if config.enabled("zp43_mixing") && raw_clean.contains("#include <zombieplague>")
        && (raw_clean.contains("#include <zp50_") || RE_ZP43_NATIVE.is_match(raw_clean).eq(&false))
        && raw_clean.contains("#include <zp50_") {
            let pos = raw_clean.find("#include <zombieplague>").unwrap();
            let lineno = raw_clean[..pos].matches('\n').count() + 1;
            issues.push(iss(lineno, "mixing ZP 4.3 API (<zombieplague>) with zp50 includes fails to load without the compat addon ('missing natives')".into(), "zp43_mixing", false));
        }
    if config.enabled("zp43_mixing") && raw_clean.contains("#include <zp50_")
        && let Some(m) = RE_ZP43_NATIVE.find(raw_clean) {
        let lineno = raw_clean[..m.start()].matches('\n').count() + 1;
        issues.push(iss(lineno, "legacy ZP 4.3 native used alongside zp50 includes - only works with the 4.3 compat addon loaded".into(), "zp43_mixing", false));
    }
}

#[cfg(test)]
mod tests {
    use crate::config::RulesConfig;
    use crate::engine::lint_file;

    fn lint_str(name: &str, content: &str) -> Vec<&'static str> {
        let path = std::env::temp_dir().join(format!("zplint_det_{}_{}.sma", name, std::process::id()));
        std::fs::write(&path, content).unwrap();
        let issues = lint_file(&path, &RulesConfig::default());
        std::fs::remove_file(path).unwrap();
        issues.into_iter().map(|i| i.rule_id).collect()
    }

    #[test]
    fn else_paren_flagged() {
        let r = lint_str("elsep", "public f(item) {\n\tif (item == 0) { a(); }\n\telse (item == 1) { b(); }\n}\n");
        assert!(r.contains(&"else_paren"));
        let ok = lint_str("elseok", "public f(item) {\n\tif (item == 0) { a(); }\n\telse if (item == 1) { b(); }\n}\n");
        assert!(!ok.contains(&"else_paren"));
    }

    #[test]
    fn string_compare_flagged() {
        let r = lint_str("strcmp1", "public f() {\n\tnew name[32];\n\tif (name == \"admin\") return 1;\n\treturn 0;\n}\n");
        assert!(r.contains(&"string_literal_compare"));
        let ok = lint_str("strcmp2", "public f() {\n\tnew name[32];\n\tif (equal(name, \"admin\")) return 1;\n\treturn 0;\n}\n");
        assert!(!ok.contains(&"string_literal_compare"));
        // == "..." inside a string literal must not fire
        let ok2 = lint_str("strcmp3", "public f(id) {\n\tclient_print(id, print_chat, \"x == \\\"y\\\"\");\n}\n");
        assert!(!ok2.contains(&"string_literal_compare"));
    }

    #[test]
    fn set_task_int_interval() {
        let r = lint_str("taskint", "public plugin_init() {\n\tset_task(10, \"tick\");\n}\npublic tick() {}\n");
        assert!(r.contains(&"set_task_int_interval"));
        let ok = lint_str("taskfloat", "public plugin_init() {\n\tset_task(10.0, \"tick\");\n}\npublic tick() {}\n");
        assert!(!ok.contains(&"set_task_int_interval"));
    }

    #[test]
    fn pev_float_int() {
        let r = lint_str("pevint", "public f(id) {\n\tset_pev(id, pev_health, 100)\n}\n");
        assert!(r.contains(&"pev_float_int"));
        let ok = lint_str("pevfloat", "public f(id) {\n\tset_pev(id, pev_health, 100.0)\n}\n");
        assert!(!ok.contains(&"pev_float_int"));
    }

    #[test]
    fn int_native_float() {
        let r = lint_str("intfl", "public f(id) {\n\tset_user_health(id, 100.0)\n}\n");
        assert!(r.contains(&"int_native_float"));
        let ok = lint_str("intok", "public f(id) {\n\tset_user_health(id, 100)\n}\n");
        assert!(!ok.contains(&"int_native_float"));
    }

    #[test]
    fn engfunc_int_float() {
        let r = lint_str("engf1", "public f(id) {\n\tengfunc(EngFunc_WriteByte, 1.0)\n}\n");
        assert!(r.contains(&"engfunc_int_float"));
        let r2 = lint_str("engf2", "public f(id, Float:origin[3], Float:dist) {\n\tengfunc(EngFunc_WalkMove, id, origin, dist, 1.0)\n}\n");
        assert!(r2.contains(&"engfunc_int_float"));
        let ok = lint_str("engfok", "public f(id) {\n\tengfunc(EngFunc_WriteByte, 1)\n}\n");
        assert!(!ok.contains(&"engfunc_int_float"));
        let ok2 = lint_str("engfok2", "public f(id, Float:origin[3], Float:dist) {\n\tengfunc(EngFunc_WalkMove, id, origin, dist, WALKMOVE_NORMAL)\n}\n");
        assert!(!ok2.contains(&"engfunc_int_float"));
    }

    #[test]
    fn engfunc_float_int() {
        // int literal into a Float parameter -> flagged
        let r = lint_str("engfi1", "public f(id) {\n\tengfunc(EngFunc_SetClientMaxspeed, id, 300)\n}\n");
        assert!(r.contains(&"engfunc_float_int"));
        // 12-arg PlaybackEvent: fparam1 (Float, position 7) as a bare int literal -> flagged
        let r2 = lint_str("engfi2", "public f() {\n\tnew Float:o[3]; new Float:a[3];\n\tengfunc(EngFunc_PlaybackEvent, 0, 0, 0, 0.1, o, a, 5, 0.0, 0, 0, 0, 0)\n}\n");
        assert!(r2.contains(&"engfunc_float_int"));
        // correctly typed -> not flagged
        let ok = lint_str("engfiok", "public f(id) {\n\tengfunc(EngFunc_SetClientMaxspeed, id, 300.0)\n}\n");
        assert!(!ok.contains(&"engfunc_float_int"));
        // bare int literal 0 is bit-identical to 0.0 - exempt
        let ok2 = lint_str("engfiok2", "public f(id) {\n\tengfunc(EngFunc_SetClientMaxspeed, id, 0)\n}\n");
        assert!(!ok2.contains(&"engfunc_float_int"));
    }

    #[test]
    fn entity_ev_type_mismatch() {
        // EV_FL_ (Float family) field passed to the int native -> flagged
        let r = lint_str("evmis1", "public f(id) {\n\tentity_set_int(id, EV_FL_gravity, 2)\n}\n");
        assert!(r.contains(&"entity_ev_type_mismatch"));
        // EV_INT_ field passed to the float native -> flagged
        let r2 = lint_str("evmis2", "public f(id) {\n\tentity_set_float(id, EV_INT_effects, 1.0)\n}\n");
        assert!(r2.contains(&"entity_ev_type_mismatch"));
        // EV_VEC_ field passed to entity_get_int -> flagged
        let r3 = lint_str("evmis3", "public f(id) {\n\tnew x = entity_get_int(id, EV_VEC_origin)\n}\n");
        assert!(r3.contains(&"entity_ev_type_mismatch"));
        // correctly matched family -> not flagged
        let ok = lint_str("evok", "public f(id) {\n\tentity_set_float(id, EV_FL_gravity, 2.0)\n\tentity_set_int(id, EV_INT_effects, 1)\n}\n");
        assert!(!ok.contains(&"entity_ev_type_mismatch"));
    }

    #[test]
    fn ham_param_mismatch() {
        // int literal into Ham_TakeDamage's Float damage slot -> flagged
        let r = lint_str("ham1", "public f(victim, id) {\n\tExecuteHamB(Ham_TakeDamage, victim, id, id, 25, DMG_ACID)\n}\n");
        assert!(r.contains(&"ham_float_int"));
        // float literal into Ham_Use's int use_type slot -> flagged
        let r2 = lint_str("ham2", "public f(ent, a, b) {\n\tExecuteHam(Ham_Use, ent, a, b, 1.0, 300.0)\n}\n");
        assert!(r2.contains(&"ham_int_float"));
        // correctly typed -> not flagged
        let ok = lint_str("hamok", "public f(victim, id) {\n\tExecuteHamB(Ham_TakeDamage, victim, id, id, 25.0, DMG_ACID)\n}\n");
        assert!(!ok.contains(&"ham_float_int"));
        assert!(!ok.contains(&"ham_int_float"));
        // a variable/constant argument can't be statically checked - must not flag
        let ok2 = lint_str("hamokvar", "public f(victim, id, Float:dmg) {\n\tExecuteHamB(Ham_TakeDamage, victim, id, id, dmg, DMG_ACID)\n}\n");
        assert!(!ok2.contains(&"ham_float_int"));
        assert!(!ok2.contains(&"ham_int_float"));
    }

    #[test]
    fn cs_float_int_rule() {
        let r = lint_str("csfi1", "public f(id) {\n\tcs_set_c4_explode_time(id, 10)\n}\n");
        assert!(r.contains(&"cs_float_int"));
        let r2 = lint_str("csfi2", "public f(id) {\n\tcs_set_user_lastactivity(id, 5)\n}\n");
        assert!(r2.contains(&"cs_float_int"));
        let ok = lint_str("csfiok", "public f(id) {\n\tcs_set_c4_explode_time(id, 10.0)\n}\n");
        assert!(!ok.contains(&"cs_float_int"));
        // bare 0 is bit-identical to 0.0 - exempt
        let ok2 = lint_str("csfiok2", "public f(id) {\n\tcs_set_c4_explode_time(id, 0)\n}\n");
        assert!(!ok2.contains(&"cs_float_int"));
    }

    #[test]
    fn fun_float_int_rule() {
        let r = lint_str("funfi1", "public f(id) {\n\tset_user_gravity(id, 1)\n}\n");
        assert!(r.contains(&"fun_float_int"));
        let r2 = lint_str("funfi2", "public f(id) {\n\tset_user_maxspeed(id, 250)\n}\n");
        assert!(r2.contains(&"fun_float_int"));
        let ok = lint_str("funfiok", "public f(id) {\n\tset_user_gravity(id, 1.0)\n\tset_user_maxspeed(id, 250.0)\n}\n");
        assert!(!ok.contains(&"fun_float_int"));
        let ok2 = lint_str("funfiok2", "public f(id) {\n\tset_user_gravity(id, 0)\n}\n");
        assert!(!ok2.contains(&"fun_float_int"));
    }

    #[test]
    fn amxmodx_param_mismatch() {
        // fxtime (param #7) is Float - bare int literal is wrong
        let r = lint_str("amx1", "public f(id) {\n\tset_hudmessage(255, 0, 0, -1.0, 0.35, 0, 6, 12.0, 0.1, 0.2)\n}\n");
        assert!(r.contains(&"amxmodx_float_int"));
        // x (param #4) is Float - float literal is correct, but flip channel(int, #6) to a float -> wrong
        let r2 = lint_str("amx2", "public f(id) {\n\temit_sound(id, 1, \"sound.wav\", 1.0, 0.8, 1.5, 100)\n}\n");
        assert!(r2.contains(&"amxmodx_int_float"));
        // change_task's newTime (param #2) is Float
        let r3 = lint_str("amx3", "public f() {\n\tchange_task(1, 5, 0)\n}\n");
        assert!(r3.contains(&"amxmodx_float_int"));
        // all correctly typed -> not flagged
        let ok = lint_str("amxok", "public f(id) {\n\tset_hudmessage(255, 0, 0, -1.0, 0.35, 0, 6.0, 12.0, 0.1, 0.2)\n\temit_sound(id, 1, \"sound.wav\", 1.0, 0.8, 0, 100)\n\tchange_task(1, 5.0, 0)\n}\n");
        assert!(!ok.contains(&"amxmodx_float_int"));
        assert!(!ok.contains(&"amxmodx_int_float"));
    }

    #[test]
    fn userid_index() {
        let r = lint_str("userid", "new g_x[33];\npublic f(id) {\n\tg_x[get_user_userid(id)]++;\n}\n");
        assert!(r.contains(&"userid_as_index"));
    }

    #[test]
    fn find_ent_no_advance() {
        let r = lint_str("fent", "public f() {\n\tnew ent;\n\twhile ((ent = find_ent_by_class(-1, \"x\"))) {\n\t\tremove_entity(ent);\n\t}\n}\n");
        assert!(r.contains(&"find_ent_no_advance"));
        let ok = lint_str("fentok", "public f() {\n\tnew ent = -1;\n\twhile ((ent = find_ent_by_class(ent, \"x\")) > 0) {\n\t\tremove_entity(ent);\n\t}\n}\n");
        assert!(!ok.contains(&"find_ent_no_advance"));
    }

    #[test]
    fn precache_mp3_and_prefix() {
        let r = lint_str("mp3", "public plugin_precache() {\n\tprecache_sound(\"music/theme.mp3\")\n}\n");
        assert!(r.contains(&"precache_mp3"));
        let r2 = lint_str("sndpre", "public plugin_precache() {\n\tprecache_sound(\"sound/zombie/pain.wav\")\n}\n");
        assert!(r2.contains(&"sound_prefix"));
        let ok = lint_str("sndok", "public plugin_precache() {\n\tprecache_sound(\"zombie/pain.wav\")\n}\n");
        assert!(!ok.contains(&"sound_prefix") && !ok.contains(&"precache_mp3"));
    }

    #[test]
    fn te_reliable() {
        let r = lint_str("terel", "public f(id) {\n\tmessage_begin(MSG_ONE, SVC_TEMPENTITY, {0,0,0}, id)\n\tmessage_end()\n}\n");
        assert!(r.contains(&"te_reliable"));
        let ok = lint_str("terelok", "public f() {\n\tmessage_begin(MSG_BROADCAST, SVC_TEMPENTITY)\n\tmessage_end()\n}\n");
        assert!(!ok.contains(&"te_reliable"));
    }

    #[test]
    fn assignment_in_condition_rule() {
        let r = lint_str("asgn", "public f(x) {\n\tif (x = 1) return 1;\n\treturn 0;\n}\n");
        assert!(r.contains(&"assignment_in_condition"));
        let ok = lint_str("asgnok", "public f(x) {\n\tif (x == 1) return 1;\n\tif ((x = other())) return 2;\n\treturn 0;\n}\n");
        assert!(!ok.contains(&"assignment_in_condition"));
        let ok2 = lint_str("asgnok2", "public f(x) {\n\tif (x >= 1) return 1;\n\treturn 0;\n}\n");
        assert!(!ok2.contains(&"assignment_in_condition"));
    }

    #[test]
    fn self_assignment_rule() {
        let r = lint_str("selfa", "public f(id) {\n\tg_class[id] = g_class[id];\n}\n");
        assert!(r.contains(&"self_assignment"));
        let ok = lint_str("selfok", "public f(id) {\n\tg_class[id] = g_next[id];\n}\n");
        assert!(!ok.contains(&"self_assignment"));
    }

    #[test]
    fn comparison_as_statement_rule() {
        let r = lint_str("cmpst", "public f() {\n\tg_mode == 5;\n}\n");
        assert!(r.contains(&"comparison_as_statement"));
        let ok = lint_str("cmpok", "public f() {\n\tif (g_mode == 5) return;\n}\n");
        assert!(!ok.contains(&"comparison_as_statement"));
    }

    #[test]
    fn infect_lasthuman_survivor_rule() {
        // Infection-by-damage handler with neither guard -> flagged
        let bad = lint_str("infbad", "public fw_TakeDamage(victim, inflictor, attacker, Float:damage, dtype) {\n\tif (zp_core_is_zombie(attacker) && !zp_core_is_zombie(victim))\n\t\tzp_core_infect(victim, attacker);\n}\n");
        assert!(bad.contains(&"zp_infect_lasthuman_survivor"));
        // Both guards present -> not flagged
        let ok = lint_str("infok", "public fw_TakeDamage(victim, inflictor, attacker, Float:damage, dtype) {\n\tif (zp_class_survivor_get(victim)) return HAM_IGNORED;\n\tif (zp_core_get_human_count() == 1) return HAM_IGNORED;\n\tzp_core_infect(victim, attacker);\n}\n");
        assert!(!ok.contains(&"zp_infect_lasthuman_survivor"));
        // Handler that does not infect -> not flagged
        let noinf = lint_str("infnone", "public fw_TakeDamage(victim, inflictor, attacker, Float:damage, dtype) {\n\tif (!is_user_alive(attacker)) return HAM_IGNORED;\n\treturn HAM_IGNORED;\n}\n");
        assert!(!noinf.contains(&"zp_infect_lasthuman_survivor"));
    }

    #[test]
    fn contain_and_strcmp_truthy() {
        let r = lint_str("cont", "public f() {\n\tnew msg[64];\n\tif (contain(msg, \"admin\")) return 1;\n\treturn 0;\n}\n");
        assert!(r.contains(&"contain_truthy"));
        let ok = lint_str("contok", "public f() {\n\tnew msg[64];\n\tif (contain(msg, \"admin\") != -1) return 1;\n\treturn 0;\n}\n");
        assert!(!ok.contains(&"contain_truthy"));
        let r2 = lint_str("strc", "public f() {\n\tnew a[8], b[8];\n\tif (strcmp(a, b)) return 1;\n\treturn 0;\n}\n");
        assert!(r2.contains(&"strcmp_truthy"));
        let ok2 = lint_str("strcok", "public f() {\n\tnew a[8], b[8];\n\tif (strcmp(a, b) == 0) return 1;\n\treturn 0;\n}\n");
        assert!(!ok2.contains(&"strcmp_truthy"));
    }

    #[test]
    fn sql_fieldname_truthy_rule() {
        let r = lint_str("sqlfn", "public f(query) {\n\tif (SQL_FieldNameToNum(query, \"id\")) return 1;\n\treturn 0;\n}\n");
        assert!(r.contains(&"sql_fieldname_truthy"));
        let ok = lint_str("sqlfnok", "public f(query) {\n\tif (SQL_FieldNameToNum(query, \"id\") != -1) return 1;\n\treturn 0;\n}\n");
        assert!(!ok.contains(&"sql_fieldname_truthy"));
        // assigning to a variable first (the common real-world idiom) is not a bare-truthy use
        let ok2 = lint_str("sqlfnvar", "public f(query) {\n\tnew col = SQL_FieldNameToNum(query, \"id\");\n\treturn col;\n}\n");
        assert!(!ok2.contains(&"sql_fieldname_truthy"));
    }

    #[test]
    fn func_id_truthy_rule() {
        // direct bare call in the condition
        let r = lint_str("fid1", "public f(name[]) {\n\tif (get_xvar_id(name)) return 1;\n\treturn 0;\n}\n");
        assert!(r.contains(&"func_id_truthy"));
        // real-world idiom: assign to a variable, then test it bare a couple lines later
        // (regression case from meus_plugins_organizados/plmenu.sma)
        let r2 = lint_str("fid2", "public f(name[]) {\n\tnew x = get_xvar_id(name);\n\tif (x) {\n\t\treturn 1;\n\t}\n\treturn 0;\n}\n");
        assert!(r2.contains(&"func_id_truthy"));
        // get_func_id, negated bare check
        let r3 = lint_str("fid3", "public f(cb[]) {\n\tnew fid = get_func_id(cb);\n\tif (!fid) return 0;\n\treturn 1;\n}\n");
        assert!(r3.contains(&"func_id_truthy"));
        // correctly compared with != -1 -> not flagged
        let ok = lint_str("fidok", "public f(name[]) {\n\tnew x = get_xvar_id(name);\n\tif (x != -1) {\n\t\treturn 1;\n\t}\n\treturn 0;\n}\n");
        assert!(!ok.contains(&"func_id_truthy"));
        let ok2 = lint_str("fidok2", "public f(name[]) {\n\tif (get_xvar_id(name) != -1) return 1;\n\treturn 0;\n}\n");
        assert!(!ok2.contains(&"func_id_truthy"));
    }

    #[test]
    fn formatex_self_rule() {
        let r = lint_str("fmx", "public f() {\n\tnew buf[64];\n\tformatex(buf, charsmax(buf), \"prefix %s\", buf);\n}\n");
        assert!(r.contains(&"formatex_self"));
        let ok = lint_str("fmxok", "public f() {\n\tnew buf[64], src[64];\n\tformatex(buf, charsmax(buf), \"prefix %s\", src);\n}\n");
        assert!(!ok.contains(&"formatex_self"));
    }

    #[test]
    fn unbalanced_preprocessor_rule() {
        let r = lint_str("ppbad", "#if defined X\nnew g_a;\npublic plugin_init() {\n}\n");
        assert!(r.contains(&"unbalanced_preprocessor"));
        let ok = lint_str("ppok", "#if defined X\nnew g_a;\n#endif\npublic plugin_init() {\n}\n");
        assert!(!ok.contains(&"unbalanced_preprocessor"));
    }

    #[test]
    fn unbalanced_braces_rule() {
        let r = lint_str("brbad", "public plugin_init() {\n\tregister_plugin(\"x\", \"1\", \"a\");\n\npublic other() {\n}\n");
        assert!(r.contains(&"unbalanced_braces"));
        let ok = lint_str("brok", "public plugin_init() {\n\tregister_plugin(\"x\", \"1\", \"a\");\n}\n");
        assert!(!ok.contains(&"unbalanced_braces"));
        // braces inside strings/chars must not count
        let ok2 = lint_str("brstr", "public f() {\n\tnew c = '{';\n\tclient_print(0, print_chat, \"{ %d }\", c);\n}\n");
        assert!(!ok2.contains(&"unbalanced_braces"));
    }

    #[test]
    fn unterminated_string_rule() {
        let r = lint_str("unterm", "public f(id) {\n\tclient_print(id, print_chat, \"Welcome!);\n}\n");
        assert!(r.contains(&"unterminated_string"));
        let ok = lint_str("untermok", "public f(id) {\n\tclient_print(id, print_chat, \"Welcome ^\"quoted^\"!\");\n}\n");
        assert!(!ok.contains(&"unterminated_string"));
    }

    #[test]
    fn player_array_32_rule() {
        let r = lint_str("p32", "new g_hp[32];\npublic f(id) {\n\tg_hp[id] = 100;\n}\n");
        assert!(r.contains(&"player_array_32"));
        let ok = lint_str("p33", "new g_hp[33];\npublic f(id) {\n\tg_hp[id] = 100;\n}\n");
        assert!(!ok.contains(&"player_array_32"));
        let ok2 = lint_str("p32i", "new g_slots[32];\npublic f() {\n\tfor (new i = 0; i < 32; i++) g_slots[i] = 0;\n}\n");
        assert!(!ok2.contains(&"player_array_32"));
    }

    #[test]
    fn forward_arity_rule() {
        let r = lint_str("arity", "public plugin_init(id) {\n}\n");
        assert!(r.contains(&"forward_arity"));
        let r2 = lint_str("arity2", "public client_putinserver(id, extra) {\n}\n");
        assert!(r2.contains(&"forward_arity"));
        let ok = lint_str("arityok", "public plugin_init() {\n}\npublic client_putinserver(id) {\n}\n");
        assert!(!ok.contains(&"forward_arity"));
    }

    #[test]
    fn stacked_case_rule() {
        let r = lint_str("scase", "public f(w) {\n\tswitch (w) {\n\t\tcase 1:\n\t\tcase 2: g();\n\t}\n}\n");
        assert!(r.contains(&"stacked_case"));
        let ok = lint_str("scaseok", "public f(w) {\n\tswitch (w) {\n\t\tcase 1, 2: g();\n\t}\n}\n");
        assert!(!ok.contains(&"stacked_case"));
    }

    #[test]
    fn global_shadowing_rule() {
        let r = lint_str("shadow", "new g_count;\npublic f() {\n\tnew g_count = 1;\n\tg_count++;\n}\n");
        assert!(r.contains(&"global_shadowing"));
        let ok = lint_str("shadowok", "new g_count;\npublic f() {\n\tnew local = 1;\n\tg_count += local;\n}\n");
        assert!(!ok.contains(&"global_shadowing"));
    }

    #[test]
    fn loops_context_rules() {
        let r = lint_str("bufloop", "public f() {\n\tfor (new i = 0; i < 32; i++) {\n\t\tnew name[64];\n\t\tget_user_name(i, name, charsmax(name));\n\t}\n}\n");
        assert!(r.contains(&"buffer_in_loop"));
        let r2 = lint_str("rfloop", "public f() {\n\tnew buf[128], len;\n\tfor (new i = 0; i < 10; i++) {\n\t\tread_file(\"x.txt\", i, buf, charsmax(buf), len);\n\t}\n}\n");
        assert!(r2.contains(&"read_file_loop"));
        let ok = lint_str("bufok", "public f() {\n\tnew name[64];\n\tfor (new i = 0; i < 32; i++) {\n\t\tget_user_name(i, name, charsmax(name));\n\t}\n}\n");
        assert!(!ok.contains(&"buffer_in_loop"));
    }

    #[test]
    fn get_cvar_hotpath_rule() {
        let r = lint_str("cvarhot", "public plugin_init() {\n\tregister_event(\"DeathMsg\", \"ev_death\", \"a\");\n}\npublic ev_death() {\n\tif (get_cvar_num(\"zp_on\")) return;\n}\n");
        assert!(r.contains(&"get_cvar_hotpath"));
        let ok = lint_str("cvarok", "public plugin_init() {\n\tnew v = get_cvar_num(\"zp_on\");\n}\n");
        assert!(!ok.contains(&"get_cvar_hotpath"));
    }

    #[test]
    fn deathmsg_killer_rule() {
        let r = lint_str("dmsg", "new g_kills[33];\npublic plugin_init() {\n\tregister_event(\"DeathMsg\", \"ev_death\", \"a\");\n}\npublic ev_death() {\n\tnew killer = read_data(1);\n\tg_kills[killer]++;\n}\n");
        assert!(r.contains(&"deathmsg_killer_guard"));
        let ok = lint_str("dmsgok", "new g_kills[33];\npublic plugin_init() {\n\tregister_event(\"DeathMsg\", \"ev_death\", \"a\");\n}\npublic ev_death() {\n\tnew killer = read_data(1);\n\tif (!is_user_connected(killer)) return;\n\tg_kills[killer]++;\n}\n");
        assert!(!ok.contains(&"deathmsg_killer_guard"));
    }

    #[test]
    fn zp50_rules() {
        let r = lint_str("zpreg", "public plugin_precache() {\n\tzp_items_register(\"Trip Mine\", 20);\n}\n");
        assert!(r.contains(&"zp50_register_return"));
        let ok = lint_str("zpregok", "new g_item;\npublic plugin_precache() {\n\tg_item = zp_items_register(\"Trip Mine\", 20);\n}\n");
        assert!(!ok.contains(&"zp50_register_return"));

        let r2 = lint_str("zpatt", "public zp_fw_core_infect(id, attacker) {\n\tzp_ammopacks_set(attacker, zp_ammopacks_get(attacker) + 5);\n}\n");
        assert!(r2.contains(&"zp_fw_attacker_guard"));
        let ok2 = lint_str("zpattok", "public zp_fw_core_infect(id, attacker) {\n\tif (!attacker || !is_user_connected(attacker)) return;\n\tzp_ammopacks_set(attacker, zp_ammopacks_get(attacker) + 5);\n}\n");
        assert!(!ok2.contains(&"zp_fw_attacker_guard"));

        let r3 = lint_str("zpsel", "public zp_fw_items_select_pre(id, itemid, ignorecost) {\n\tif (zp_core_is_zombie(id))\n\t\treturn ZP_ITEM_DONT_SHOW;\n\treturn ZP_ITEM_AVAILABLE;\n}\n");
        assert!(r3.contains(&"zp_select_pre_filter"));
        let ok3 = lint_str("zpselok", "new g_item;\npublic zp_fw_items_select_pre(id, itemid, ignorecost) {\n\tif (itemid != g_item)\n\t\treturn ZP_ITEM_AVAILABLE;\n\tif (zp_core_is_zombie(id))\n\t\treturn ZP_ITEM_DONT_SHOW;\n\treturn ZP_ITEM_AVAILABLE;\n}\n");
        assert!(!ok3.contains(&"zp_select_pre_filter"));
    }

    #[test]
    fn unreachable_code_rule() {
        let r = lint_str("unreach", "public f(id) {\n\treturn PLUGIN_HANDLED;\n\tclient_print(id, print_chat, \"x\");\n}\n");
        assert!(r.contains(&"unreachable_code"));
        let ok = lint_str("unreachok", "public f(id) {\n\tif (id)\n\t\treturn PLUGIN_HANDLED;\n\tclient_print(id, print_chat, \"x\");\n\treturn PLUGIN_CONTINUE;\n}\n");
        assert!(!ok.contains(&"unreachable_code"));
        let ok2 = lint_str("unreach2", "public f(id) {\n\tswitch (id) {\n\t\tcase 1: return 1;\n\t\tcase 2: return 2;\n\t}\n\treturn 0;\n}\n");
        assert!(!ok2.contains(&"unreachable_code"));
    }

    #[test]
    fn format_injection_rule() {
        let r = lint_str("fmtinj", "public f(id) {\n\tnew said[192];\n\tread_args(said, charsmax(said));\n\tclient_print(0, print_chat, said);\n}\n");
        assert!(r.contains(&"format_injection"));
        let ok = lint_str("fmtok", "public f(id) {\n\tnew said[192];\n\tread_args(said, charsmax(said));\n\tclient_print(0, print_chat, \"%s\", said);\n}\n");
        assert!(!ok.contains(&"format_injection"));
    }

    #[test]
    fn empty_statement_and_dowhile() {
        let r = lint_str("empt", "public f(id) {\n\tif (is_user_alive(id));\n\t\tuser_kill(id);\n}\n");
        assert!(r.contains(&"empty_statement"));
        let ok = lint_str("dowhile", "public f() {\n\tnew i = 0;\n\tdo {\n\t\ti++;\n\t}\n\twhile (i < 3);\n}\n");
        assert!(!ok.contains(&"empty_statement"));
    }

    #[test]
    fn client_command_handled_rule() {
        let r = lint_str("cch", "public client_command(id) {\n\tif (id) {\n\t\treturn PLUGIN_HANDLED;\n\t}\n\treturn PLUGIN_CONTINUE;\n}\n");
        assert!(r.contains(&"client_command_handled"));
        let ok = lint_str("cchok", "public client_command(id) {\n\tif (id) {\n\t\treturn PLUGIN_HANDLED_MAIN;\n\t}\n\treturn PLUGIN_CONTINUE;\n}\n");
        assert!(!ok.contains(&"client_command_handled"));
    }

    #[test]
    fn model_not_precached_rule() {
        let r = lint_str("mdl", "public fw_spawn(ent) {\n\tentity_set_model(ent, \"models/custom/crate.mdl\")\n}\n");
        assert!(r.contains(&"model_not_precached"));
        let ok = lint_str("mdlok", "public plugin_precache() {\n\tprecache_model(\"models/custom/crate.mdl\")\n}\npublic fw_spawn(ent) {\n\tentity_set_model(ent, \"models/custom/crate.mdl\")\n}\n");
        assert!(!ok.contains(&"model_not_precached"));
    }

    #[test]
    fn entity_leak_rule() {
        let r = lint_str("leak", "public fw_kill(id) {\n\tnew ent = create_entity(\"info_target\");\n\tif (!ent) return;\n}\n");
        assert!(r.contains(&"entity_leak"));
        let ok = lint_str("leakok", "public fw_kill(id) {\n\tnew ent = create_entity(\"info_target\");\n\tif (!ent) return;\n\tremove_entity(ent);\n}\n");
        assert!(!ok.contains(&"entity_leak"));
    }

    #[test]
    fn callback_not_defined_rule() {
        let r = lint_str("cbnd", "public plugin_init() {\n\tregister_clcmd(\"say /vip\", \"cmd_vip\");\n}\npublic cmdVip(id) {\n\treturn PLUGIN_HANDLED;\n}\n");
        assert!(r.contains(&"callback_not_defined"));
        let ok = lint_str("cbndok", "public plugin_init() {\n\tregister_clcmd(\"say /vip\", \"cmd_vip\");\n}\npublic cmd_vip(id) {\n\treturn PLUGIN_HANDLED;\n}\n");
        assert!(!ok.contains(&"callback_not_defined"));
    }

    #[test]
    fn deprecated_and_define_rules() {
        let r = lint_str("depr", "public f() {\n\tnew hash[34];\n\tmd5(\"x\", hash);\n}\n");
        assert!(r.contains(&"deprecated_symbols"));
        let r2 = lint_str("defres", "#define MAX_PLAYERS 32\nnew g_hp[MAX_PLAYERS + 1];\n");
        assert!(r2.contains(&"define_reserved_const"));
    }

    #[test]
    fn geoip_code_overflow_rule() {
        let r = lint_str("geo1", "public f(ip[16]) {\n\tnew code[3];\n\tgeoip_code2(ip, code);\n}\n");
        assert!(r.contains(&"geoip_code_overflow"));
        let r2 = lint_str("geo2", "public f(ip[16]) {\n\tnew code[4];\n\tgeoip_code3(ip, code);\n}\n");
        assert!(r2.contains(&"geoip_code_overflow"));
        // the _ex variants are the documented-safe replacement - must not flag
        let ok = lint_str("geook", "public f(ip[16]) {\n\tnew code[3];\n\tgeoip_code2_ex(ip, code);\n}\n");
        assert!(!ok.contains(&"geoip_code_overflow"));
    }

    #[test]
    fn string_assign_rule() {
        let r = lint_str("strassign", "public f() {\n\tnew msg[8];\n\tmsg = \"Hello World!\";\n}\n");
        assert!(r.contains(&"string_assign"));
        let ok = lint_str("strassignok", "public f() {\n\tnew msg[16];\n\tcopy(msg, charsmax(msg), \"Hello World!\");\n}\n");
        assert!(!ok.contains(&"string_assign"));
        // a literal that fits is legal Pawn
        let ok3 = lint_str("strassignfits", "public f() {\n\tnew msg[16];\n\tmsg = \"Hello\";\n}\n");
        assert!(!ok3.contains(&"string_assign"));
        let ok2 = lint_str("strassigndecl", "new g_prefix[] = \"[ZP]\";\npublic f() {\n}\n");
        assert!(!ok2.contains(&"string_assign"));
    }

    #[test]
    fn array_index_oob_rule() {
        // exact size as index - classic off-by-one (valid range is 0..size-1)
        let r = lint_str("oob1", "public f() {\n\tnew Players[32];\n\tPlayers[32] = 15;\n}\n");
        assert!(r.contains(&"array_index_oob"));
        // negative literal index
        let r2 = lint_str("oob2", "public f() {\n\tnew Players[32];\n\tPlayers[-1] = 6;\n}\n");
        assert!(r2.contains(&"array_index_oob"));
        // in-bounds access -> not flagged
        let ok = lint_str("oobok", "public f() {\n\tnew Players[32];\n\tPlayers[31] = 15;\n}\n");
        assert!(!ok.contains(&"array_index_oob"));
        // the declaration line itself must never self-flag
        let ok2 = lint_str("oobdecl", "public f() {\n\tnew Players[32];\n}\n");
        assert!(!ok2.contains(&"array_index_oob"));
        // dynamic (variable) index can't be statically checked - must not flag
        let ok3 = lint_str("oobdyn", "public f(i) {\n\tnew Players[32];\n\tPlayers[i] = 15;\n}\n");
        assert!(!ok3.contains(&"array_index_oob"));
        // regression: a later array in a multi-var `new` statement has no leading `new` of
        // its own (`new name[32], authid[32]`) - must not be misread as an access to itself
        let ok4 = lint_str("oobmultidecl", "public f() {\n\tnew name[32], authid[32];\n}\n");
        assert!(!ok4.contains(&"array_index_oob"));
        // regression: an array parameter in a function signature is a declaration too
        let ok5 = lint_str("oobsig", "public f(id, msg[128]) {\n\treturn 1;\n}\n");
        assert!(!ok5.contains(&"array_index_oob"));
        // a comparison (read, not write) is intentionally out of scope - no false negative claim
        let ok6 = lint_str("oobread", "public f() {\n\tnew Players[32];\n\tif (Players[32] > 0) {\n\t\treturn 1;\n\t}\n\treturn 0;\n}\n");
        assert!(!ok6.contains(&"array_index_oob"));
    }

    #[test]
    fn array_compare_by_ref_rule() {
        let r = lint_str("arrcmp", "public f() {\n\tnew arrayOne[3];\n\tnew arrayTwo[3];\n\tif (arrayOne == arrayTwo) {\n\t\treturn 1;\n\t}\n\treturn 0;\n}\n");
        assert!(r.contains(&"array_compare_by_ref"));
        // element-by-element compare is the correct idiom -> not flagged
        let ok = lint_str("arrcmpok", "public f() {\n\tnew arrayOne[3];\n\tnew arrayTwo[3];\n\tif (arrayOne[0] == arrayTwo[0]) {\n\t\treturn 1;\n\t}\n\treturn 0;\n}\n");
        assert!(!ok.contains(&"array_compare_by_ref"));
        // scalar comparison must never be flagged
        let ok2 = lint_str("arrcmpscalar", "public f(a, b) {\n\tif (a == b) {\n\t\treturn 1;\n\t}\n\treturn 0;\n}\n");
        assert!(!ok2.contains(&"array_compare_by_ref"));
    }

    #[test]
    fn div_by_runtime_rule() {
        let r = lint_str("div", "public f(total) {\n\tnew share = total / get_playersnum();\n\treturn share;\n}\n");
        assert!(r.contains(&"div_by_runtime"));
        // '%' and '/' inside strings must not fire
        let ok = lint_str("divok", "public f(id) {\n\tclient_print(id, print_chat, \"hp: %d / max\", 100);\n}\n");
        assert!(!ok.contains(&"div_by_runtime"));
    }

    #[test]
    fn set_ham_param_mismatch_rule() {
        // Ham_TakeDamage's parameter #4 is Float:damage - SetHamParamInteger there is wrong
        let bad = lint_str("shp1",
            "public plugin_init() {\n\tRegisterHam(Ham_TakeDamage, \"player\", \"fw_Dmg\", 0);\n}\npublic fw_Dmg(this, inflictor, attacker, Float:damage, dt) {\n\tSetHamParamInteger(4, 100);\n\treturn HAM_HANDLED;\n}\n");
        assert!(bad.contains(&"set_ham_param_mismatch"));
        // parameter #3 (idattacker) is int/entity - SetHamParamFloat there is wrong
        let bad2 = lint_str("shp2",
            "public plugin_init() {\n\tRegisterHam(Ham_TakeDamage, \"player\", \"fw_Dmg2\", 0);\n}\npublic fw_Dmg2(this, inflictor, attacker, Float:damage, dt) {\n\tSetHamParamFloat(3, 0.0);\n\treturn HAM_HANDLED;\n}\n");
        assert!(bad2.contains(&"set_ham_param_mismatch"));
        // correctly matched setter/position pairs -> not flagged
        let ok = lint_str("shpok",
            "public plugin_init() {\n\tRegisterHam(Ham_TakeDamage, \"player\", \"fw_Dmg3\", 0);\n}\npublic fw_Dmg3(this, inflictor, attacker, Float:damage, dt) {\n\tSetHamParamFloat(4, 25.0);\n\tSetHamParamInteger(3, 2);\n\treturn HAM_HANDLED;\n}\n");
        assert!(!ok.contains(&"set_ham_param_mismatch"));
        // regression: Ham_TraceAttack's Float:direction[3] (forward param #4) occupies THREE
        // consecutive which-slots (4,5,6), one per vector component - not a single slot
        // followed by int slots. Real corpus bug: an earlier table version flagged this.
        let ok2 = lint_str("shpvec",
            "public plugin_init() {\n\tRegisterHam(Ham_TraceAttack, \"player\", \"fw_Trace\", 0);\n}\npublic fw_Trace(victim, attacker, Float:damage, Float:direction[3], tracehandle, damage_type) {\n\tSetHamParamFloat(4, direction[0]);\n\tSetHamParamFloat(5, direction[1]);\n\tSetHamParamFloat(6, direction[2]);\n\treturn HAM_IGNORED;\n}\n");
        assert!(!ok2.contains(&"set_ham_param_mismatch"));
    }

    #[test]
    fn remove_entity_in_damage_hook_rule() {
        // synchronous remove_entity inside a TakeDamage hook -> flagged
        let bad = lint_str("rmdmg",
            "public plugin_init() {\n\tRegisterHam(Ham_TakeDamage, \"info_target\", \"fw_Dmg\");\n}\npublic fw_Dmg(ent, i, a, Float:d, dt) {\n\tremove_entity(ent);\n\treturn HAM_SUPERCEDE;\n}\n");
        assert!(bad.contains(&"remove_entity_in_damage_hook"));
        // deferred removal (remove lives in the task callback) -> not flagged
        let ok = lint_str("rmdmgok",
            "public plugin_init() {\n\tRegisterHam(Ham_TakeDamage, \"info_target\", \"fw_Dmg\");\n}\npublic fw_Dmg(ent, i, a, Float:d, dt) {\n\tset_task(0.1, \"task_rm\", ent);\n\treturn HAM_SUPERCEDE;\n}\npublic task_rm(ent) {\n\tremove_entity(ent);\n}\n");
        assert!(!ok.contains(&"remove_entity_in_damage_hook"));
        // FL_KILLME (safe engine-deferred) -> not flagged
        let ok2 = lint_str("rmdmgkill",
            "public plugin_init() {\n\tRegisterHam(Ham_TakeDamage, \"info_target\", \"fw_Dmg2\");\n}\npublic fw_Dmg2(ent, i, a, Float:d, dt) {\n\tset_pev(ent, pev_flags, pev(ent, pev_flags) | FL_KILLME);\n\treturn HAM_SUPERCEDE;\n}\n");
        assert!(!ok2.contains(&"remove_entity_in_damage_hook"));
        // Ham_Touch self-remove (ubiquitous safe pickup idiom) -> intentionally NOT flagged
        let ok3 = lint_str("rmtouch",
            "public plugin_init() {\n\tRegisterHam(Ham_Touch, \"info_target\", \"fw_Tch\");\n}\npublic fw_Tch(ent, other) {\n\tremove_entity(ent);\n}\n");
        assert!(!ok3.contains(&"remove_entity_in_damage_hook"));
    }
}
