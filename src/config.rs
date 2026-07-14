use serde::Deserialize;

static CONFIG_FILE: &str = "zplint.toml";

#[derive(Deserialize, Clone)]
pub struct Config {
    #[serde(default = "default_paths")]
    pub paths: Vec<String>,
    #[serde(default = "default_exclude")]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub rules: RulesConfig,
    #[serde(default)]
    pub output: OutputConfig,
}

#[derive(Deserialize, Clone)]
pub struct RulesConfig {
    // Existing 17 rules
    #[serde(default = "true_val")] pub client_disconnect_guard: bool,
    #[serde(default = "true_val")] pub dangerous_forward_guard: bool,
    #[serde(default = "true_val")] pub message_begin_guard: bool,
    #[serde(default = "true_val")] pub touch_spam: bool,
    #[serde(default = "true_val")] pub precache_sound: bool,
    #[serde(default = "true_val")] pub find_entity_in_sphere: bool,
    #[serde(default = "true_val")] pub loop_player_guard: bool,
    #[serde(default = "true_val")] pub set_task_public: bool,
    #[serde(default = "true_val")] pub read_data_multi_context: bool,
    #[serde(default = "true_val")] pub zp_infect_cure_guard: bool,
    #[serde(default = "true_val")] pub zp_gamemode_if: bool,
    #[serde(default = "true_val")] pub zp_class_if: bool,
    #[serde(default = "true_val")] pub pev_oldbuttons: bool,
    #[serde(default = "true_val")] pub precache_sound_sprite: bool,
    #[serde(default = "true_val")] pub create_entity_guard: bool,
    #[serde(default = "true_val")] pub buffer_size: bool,
    #[serde(default = "true_val")] pub client_cmd_spk: bool,
    #[serde(default = "true_val")] pub zp_items_register_check: bool,
    // New rules
    #[serde(default = "true_val")] pub attacker_not_validated: bool,
    #[serde(default = "true_val")] pub get_user_origin: bool,
    #[serde(default = "true_val")] pub task_interval_zero: bool,
    #[serde(default = "true_val")] pub set_task_flags: bool,
    #[serde(default = "true_val")] pub abort_call: bool,
    #[serde(default = "true_val")] pub nested_message: bool,
    #[serde(default = "true_val")] pub message_write_outside: bool,
    #[serde(default = "true_val")] pub message_end_without_begin: bool,
    #[serde(default = "true_val")] pub message_hook_scope: bool,
    #[serde(default = "true_val")] pub hardcoded_message_id: bool,
    #[serde(default = "true_val")] pub array_random_empty: bool,
    #[serde(default = "true_val")] pub hardcoded_maxplayers: bool,
    #[serde(default = "true_val")] pub precache_outside_precache: bool,
    #[serde(default = "true_val")] pub zp_force_no_guard: bool,
    #[serde(default = "true_val")] pub library_exists_hotpath: bool,
    #[serde(default = "true_val")] pub registered_callback_public: bool,
    #[serde(default = "true_val")] pub percent_n_player_name: bool,
    #[serde(default = "true_val")] pub menu_handler_destroy: bool,
    #[serde(default = "true_val")] pub fopen_close: bool,
    /// Rule ids from detectors.rs to turn off (they are on by default).
    #[serde(default)] pub disable: Vec<String>,
}

impl RulesConfig {
    pub fn enabled(&self, rule_id: &str) -> bool {
        !self.disable.iter().any(|d| d == rule_id)
    }
}

#[derive(Deserialize, Clone, Default)]
pub struct OutputConfig {
    #[serde(default = "true_val")]
    pub color: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            paths: default_paths(),
            exclude: default_exclude(),
            rules: RulesConfig::default(),
            output: OutputConfig { color: true },
        }
    }
}

impl Default for RulesConfig {
    fn default() -> Self {
        Self {
            client_disconnect_guard: true, dangerous_forward_guard: true,
            message_begin_guard: true, touch_spam: true,
            precache_sound: true, find_entity_in_sphere: true,
            loop_player_guard: true, set_task_public: true,
            read_data_multi_context: true, zp_infect_cure_guard: true,
            zp_gamemode_if: true, zp_class_if: true,
            pev_oldbuttons: true, precache_sound_sprite: true,
            create_entity_guard: true, buffer_size: true,
            client_cmd_spk: true, zp_items_register_check: true,
            attacker_not_validated: true, get_user_origin: true,
            task_interval_zero: true, set_task_flags: true, abort_call: true,
            nested_message: true, message_write_outside: true,
            message_end_without_begin: true, message_hook_scope: true,
            hardcoded_message_id: true,
            array_random_empty: true,
            hardcoded_maxplayers: true,
            precache_outside_precache: true, zp_force_no_guard: true,
            library_exists_hotpath: true,
            registered_callback_public: true,
            percent_n_player_name: true,
            menu_handler_destroy: true,
            fopen_close: true,
            disable: Vec::new(),
        }
    }
}

fn default_paths() -> Vec<String> { vec!["meus_plugins_organizados".to_string()] }
fn default_exclude() -> Vec<String> { vec!["00-Old_Archive".to_string()] }
fn true_val() -> bool { true }

impl Config {
    pub fn load(root: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let path = root.join(CONFIG_FILE);
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            Ok(toml::from_str(&content)?)
        } else {
            Ok(Self::default())
        }
    }
}
