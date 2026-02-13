// Code generator for settings from settings.toml
// Based on the pattern from hk (https://github.com/jdx/hk)
//
// This module generates type-safe Rust structs from a settings.toml schema file.
// It creates three files in OUT_DIR/generated/:
// - settings.rs: Main Settings struct with nested sub-structs
// - settings_merge.rs: Merge types and enums for value tracking
// - settings_meta.rs: Metadata for introspection (env vars, defaults, etc.)

use heck::ToUpperCamelCase;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::fs;
use std::path::PathBuf;
use toml::Table;

pub fn generate() -> Result<(), Box<dyn std::error::Error>> {
    let settings_toml = fs::read_to_string("settings.toml")?;
    let settings: Table = settings_toml.parse()?;

    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    let generated_dir = out_dir.join("generated");
    fs::create_dir_all(&generated_dir)?;

    // Generate settings.rs - the main Settings struct and nested structs
    let settings_rs = generate_settings_struct(&settings)?;
    fs::write(generated_dir.join("settings.rs"), settings_rs)?;

    // Generate settings_merge.rs - merge types and enums
    let settings_merge_rs = generate_merge_types()?;
    fs::write(generated_dir.join("settings_merge.rs"), settings_merge_rs)?;

    // Generate settings_meta.rs - metadata for introspection
    let settings_meta_rs = generate_metadata(&settings)?;
    fs::write(generated_dir.join("settings_meta.rs"), settings_meta_rs)?;

    Ok(())
}

/// Check if a table is a leaf setting (has a "type" field) or a nested group
fn is_leaf_setting(table: &Table) -> bool {
    table.contains_key("type")
}

/// Generate all struct definitions and implementations
fn generate_settings_struct(settings: &Table) -> Result<String, Box<dyn std::error::Error>> {
    let mut tokens = TokenStream::new();

    // Generate nested structs first (depth-first)
    for (key, value) in settings {
        if let Some(table) = value.as_table()
            && !is_leaf_setting(table)
        {
            tokens.extend(generate_nested_structs(key, table)?);
        }
    }

    // Generate the main Settings struct
    tokens.extend(generate_struct("Settings", settings)?);

    // Generate Default implementations
    for (key, value) in settings {
        if let Some(table) = value.as_table()
            && !is_leaf_setting(table)
        {
            tokens.extend(generate_nested_defaults(key, table)?);
        }
    }
    tokens.extend(generate_default_impl("Settings", settings)?);

    // Generate Settings impl with load methods and env var loading
    let load_env = generate_load_from_env(settings, "self");
    let duration_helpers = generate_duration_helpers(settings);
    let merge_from = generate_merge_from(settings);

    let impl_block = quote! {
        impl Settings {
            /// Load settings from pitchfork.toml files, then overlay environment variables.
            /// Settings are loaded from all pitchfork.toml files in precedence order:
            /// 1. System-level: /etc/pitchfork/config.toml
            /// 2. User-level: ~/.config/pitchfork/config.toml
            /// 3. Project-level: pitchfork.toml files from root to current directory
            /// Environment variables override all file-based settings.
            pub fn load() -> Self {
                // Start with defaults
                let mut settings = Self::default();

                // Load and merge from all pitchfork.toml files
                // Note: We can't use PitchforkToml::all_merged() here to avoid circular dependency
                // Instead, we load each config file directly
                for path in Self::config_paths() {
                    if path.exists() {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            // Parse as a generic TOML table and extract [settings] section
                            if let Ok(table) = content.parse::<toml::Table>() {
                                if let Some(settings_table) = table.get("settings") {
                                    if let Ok(file_settings) = settings_table.clone().try_into::<Settings>() {
                                        settings.merge_from(&file_settings);
                                    }
                                }
                            }
                        }
                    }
                }

                // Environment variables override file settings
                settings.load_from_env();
                settings
            }

            /// Get all config file paths in precedence order (lowest to highest)
            fn config_paths() -> Vec<std::path::PathBuf> {
                let mut paths = Vec::new();
                paths.push(crate::env::PITCHFORK_GLOBAL_CONFIG_SYSTEM.clone());
                paths.push(crate::env::PITCHFORK_GLOBAL_CONFIG_USER.clone());

                // Find project-level pitchfork.toml files
                let mut project_paths = xx::file::find_up_all(
                    &*crate::env::CWD,
                    &["pitchfork.local.toml", "pitchfork.toml"]
                );
                project_paths.reverse();
                paths.extend(project_paths);

                paths
            }

            /// Override settings from environment variables
            pub fn load_from_env(&mut self) {
                #load_env
            }

            /// Merge settings from another Settings instance.
            /// Only non-default values from `other` will override `self`.
            pub fn merge_from(&mut self, other: &Self) {
                let defaults = Self::default();
                #merge_from
            }

            /// Parse a duration string (humantime format) to Duration
            pub fn parse_duration(s: &str) -> Option<std::time::Duration> {
                humantime::parse_duration(s).ok()
            }

            #duration_helpers
        }

        /// Global settings instance
        static SETTINGS: std::sync::OnceLock<Settings> = std::sync::OnceLock::new();

        /// Get the global settings instance
        pub fn settings() -> &'static Settings {
            SETTINGS.get_or_init(Settings::load)
        }
    };

    tokens.extend(impl_block);

    // Wrap in necessary imports
    let output = quote! {
        #[allow(unused_imports)]
        use std::time::Duration;
        #[allow(unused_imports)]
        use std::sync::OnceLock;

        #tokens
    };

    Ok(output.to_string())
}

/// Generate nested structs recursively
fn generate_nested_structs(
    prefix: &str,
    table: &Table,
) -> Result<TokenStream, Box<dyn std::error::Error>> {
    let mut tokens = TokenStream::new();

    // First, recurse into any nested groups
    for (key, value) in table {
        if let Some(child_table) = value.as_table()
            && !is_leaf_setting(child_table)
        {
            let child_prefix = format!("{}_{}", prefix, key);
            tokens.extend(generate_nested_structs(&child_prefix, child_table)?);
        }
    }

    // Then generate this struct
    let struct_name = format!("Settings{}", prefix.to_upper_camel_case());
    tokens.extend(generate_struct(&struct_name, table)?);

    Ok(tokens)
}

/// Generate a single struct definition
fn generate_struct(
    struct_name: &str,
    table: &Table,
) -> Result<TokenStream, Box<dyn std::error::Error>> {
    let struct_ident = format_ident!("{}", struct_name);
    let mut fields = Vec::new();

    for (key, value) in table {
        let field_ident = format_ident!("{}", key);

        if let Some(props) = value.as_table() {
            if is_leaf_setting(props) {
                // This is a leaf setting with a type
                let type_str = props
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("String");
                let rust_type = parse_type(type_str)?;

                // Get description for doc comment
                let doc = props
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                fields.push(quote! {
                    #[doc = #doc]
                    pub #field_ident: #rust_type
                });
            } else {
                // This is a nested group
                let child_struct_name = format!("Settings{}", key.to_upper_camel_case());
                let child_type = format_ident!("{}", child_struct_name);

                fields.push(quote! {
                    pub #field_ident: #child_type
                });
            }
        }
    }

    Ok(quote! {
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
        #[serde(default)]
        pub struct #struct_ident {
            #(#fields),*
        }
    })
}

/// Generate Default implementations for nested structs
fn generate_nested_defaults(
    prefix: &str,
    table: &Table,
) -> Result<TokenStream, Box<dyn std::error::Error>> {
    let mut tokens = TokenStream::new();

    // First, recurse into any nested groups
    for (key, value) in table {
        if let Some(child_table) = value.as_table()
            && !is_leaf_setting(child_table)
        {
            let child_prefix = format!("{}_{}", prefix, key);
            tokens.extend(generate_nested_defaults(&child_prefix, child_table)?);
        }
    }

    // Then generate default for this struct
    let struct_name = format!("Settings{}", prefix.to_upper_camel_case());
    tokens.extend(generate_default_impl(&struct_name, table)?);

    Ok(tokens)
}

/// Generate Default impl for a single struct
fn generate_default_impl(
    struct_name: &str,
    table: &Table,
) -> Result<TokenStream, Box<dyn std::error::Error>> {
    let struct_ident = format_ident!("{}", struct_name);
    let mut field_defaults = Vec::new();

    for (key, value) in table {
        let field_ident = format_ident!("{}", key);

        if let Some(props) = value.as_table() {
            if is_leaf_setting(props) {
                // Get default value and type
                let default_str = props.get("default").and_then(|v| v.as_str());
                let type_str = props
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("String");

                let default_value = if let Some(default) = default_str {
                    parse_default(default, type_str)?
                } else {
                    // Use type's default
                    match type_str {
                        "Bool" => quote! { false },
                        "Integer" => quote! { 0 },
                        "String" | "Duration" => quote! { String::new() },
                        "Path" => quote! { std::path::PathBuf::new() },
                        _ => quote! { Default::default() },
                    }
                };

                field_defaults.push(quote! {
                    #field_ident: #default_value
                });
            } else {
                // Nested struct - use its default
                let child_struct_name = format!("Settings{}", key.to_upper_camel_case());
                let child_type = format_ident!("{}", child_struct_name);
                field_defaults.push(quote! {
                    #field_ident: #child_type::default()
                });
            }
        }
    }

    Ok(quote! {
        impl Default for #struct_ident {
            fn default() -> Self {
                Self {
                    #(#field_defaults),*
                }
            }
        }
    })
}

/// Parse default value based on type
fn parse_default(default: &str, typ: &str) -> Result<TokenStream, Box<dyn std::error::Error>> {
    Ok(match typ {
        "Bool" => match default {
            "true" => quote! { true },
            "false" => quote! { false },
            _ => return Err(format!("Invalid bool default: {}", default).into()),
        },
        "Integer" => {
            let n: i64 = default
                .parse()
                .map_err(|_| format!("Invalid integer default: {}", default))?;
            quote! { #n }
        }
        "String" | "Duration" => {
            // Remove any surrounding quotes if present
            let default_str = default.trim_matches('"');
            quote! { #default_str.to_string() }
        }
        "Path" => {
            // Handle complex expressions like dirs::config_dir()...
            if default.starts_with("dirs::") || default.contains("::") {
                let tokens: TokenStream = default.parse()?;
                quote! { #tokens }
            } else {
                let default_str = default.trim_matches('"');
                quote! { std::path::PathBuf::from(#default_str) }
            }
        }
        "Url" => {
            let default_str = default.trim_matches('"');
            quote! { #default_str.to_string() }
        }
        _ => return Err(format!("Unsupported type for default: {}", typ).into()),
    })
}

/// Generate code to load settings from environment variables
fn generate_load_from_env(table: &Table, path: &str) -> TokenStream {
    let mut stmts = Vec::new();

    for (key, value) in table {
        if let Some(props) = value.as_table() {
            if is_leaf_setting(props) {
                // Check if this setting has an env var binding
                if let Some(env_var) = props.get("env").and_then(|v| v.as_str()) {
                    let type_str = props
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("String");
                    let field_path: TokenStream = format!("{}.{}", path, key).parse().unwrap();

                    let assign = match type_str {
                        "Bool" => quote! {
                            if let Ok(val) = std::env::var(#env_var) {
                                if let Ok(b) = val.parse::<bool>() {
                                    #field_path = b;
                                } else if val == "1" {
                                    #field_path = true;
                                } else if val == "0" {
                                    #field_path = false;
                                }
                            }
                        },
                        "Integer" => quote! {
                            if let Ok(val) = std::env::var(#env_var) {
                                if let Ok(n) = val.parse::<i64>() {
                                    #field_path = n;
                                }
                            }
                        },
                        "String" | "Duration" => quote! {
                            if let Ok(val) = std::env::var(#env_var) {
                                #field_path = val;
                            }
                        },
                        "Path" => quote! {
                            if let Ok(val) = std::env::var(#env_var) {
                                #field_path = std::path::PathBuf::from(val);
                            }
                        },
                        _ => quote! {},
                    };

                    stmts.push(assign);
                }
            } else {
                // Nested group - recurse
                let nested_path = format!("{}.{}", path, key);
                let nested = generate_load_from_env(props, &nested_path);
                stmts.push(nested);
            }
        }
    }

    quote! {
        #(#stmts)*
    }
}

/// Generate merge_from logic for Settings
/// This generates code that merges non-default values from `other` into `self`
fn generate_merge_from(table: &Table) -> TokenStream {
    fn generate_merge_for_group(
        table: &Table,
        self_path: &str,
        other_path: &str,
        defaults_path: &str,
    ) -> TokenStream {
        let mut stmts = Vec::new();

        for (key, value) in table {
            if let Some(props) = value.as_table() {
                let self_field: TokenStream = format!("{}.{}", self_path, key).parse().unwrap();
                let other_field: TokenStream = format!("{}.{}", other_path, key).parse().unwrap();
                let defaults_field: TokenStream =
                    format!("{}.{}", defaults_path, key).parse().unwrap();

                if is_leaf_setting(props) {
                    let type_str = props
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("String");

                    // Generate comparison based on type
                    let merge_stmt = match type_str {
                        "Bool" => quote! {
                            if #other_field != #defaults_field {
                                #self_field = #other_field;
                            }
                        },
                        "Integer" => quote! {
                            if #other_field != #defaults_field {
                                #self_field = #other_field;
                            }
                        },
                        "String" | "Duration" => quote! {
                            if #other_field != #defaults_field {
                                #self_field = #other_field.clone();
                            }
                        },
                        "Path" => quote! {
                            if #other_field != #defaults_field {
                                #self_field = #other_field.clone();
                            }
                        },
                        _ => quote! {},
                    };

                    stmts.push(merge_stmt);
                } else {
                    // Nested group - recurse
                    let nested_self = format!("{}.{}", self_path, key);
                    let nested_other = format!("{}.{}", other_path, key);
                    let nested_defaults = format!("{}.{}", defaults_path, key);
                    let nested = generate_merge_for_group(
                        props,
                        &nested_self,
                        &nested_other,
                        &nested_defaults,
                    );
                    stmts.push(nested);
                }
            }
        }

        quote! {
            #(#stmts)*
        }
    }

    generate_merge_for_group(table, "self", "other", "defaults")
}

/// Generate convenience methods for Duration fields
fn generate_duration_helpers(settings: &Table) -> TokenStream {
    let mut methods = Vec::new();

    fn collect_duration_fields(table: &Table, prefix: &str, methods: &mut Vec<TokenStream>) {
        for (key, value) in table {
            if let Some(props) = value.as_table() {
                if is_leaf_setting(props) {
                    let type_str = props
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("String");
                    if type_str == "Duration" {
                        // Create a unique method name that includes the full path
                        // e.g., "general.interval" -> "general_interval"
                        let method_name = if prefix.is_empty() {
                            format_ident!("{}", key)
                        } else {
                            format_ident!("{}_{}", prefix.replace('.', "_"), key)
                        };
                        let field_path: TokenStream = if prefix.is_empty() {
                            format!("self.{}", key).parse().unwrap()
                        } else {
                            format!("self.{}.{}", prefix, key).parse().unwrap()
                        };

                        // Get default from the field definition for fallback
                        let default_str = props
                            .get("default")
                            .and_then(|v| v.as_str())
                            .unwrap_or("1s");
                        // Remove any surrounding quotes from the default value
                        let default_duration = default_str.trim_matches('"');

                        let fallback: TokenStream = format!(
                            "humantime::parse_duration(\"{}\").unwrap_or(std::time::Duration::from_secs(1))",
                            default_duration
                        ).parse().unwrap();

                        let doc_comment = if prefix.is_empty() {
                            format!("Get `{}` as Duration", key)
                        } else {
                            format!("Get `{}.{}` as Duration", prefix, key)
                        };

                        methods.push(quote! {
                            #[doc = #doc_comment]
                            #[allow(dead_code)]
                            pub fn #method_name(&self) -> std::time::Duration {
                                Self::parse_duration(&#field_path)
                                    .unwrap_or_else(|| #fallback)
                            }
                        });
                    }
                } else {
                    // Recurse into nested groups
                    let new_prefix = if prefix.is_empty() {
                        key.to_string()
                    } else {
                        format!("{}.{}", prefix, key)
                    };
                    collect_duration_fields(props, &new_prefix, methods);
                }
            }
        }
    }

    collect_duration_fields(settings, "", &mut methods);

    quote! {
        #(#methods)*
    }
}

/// Generate merge types and enums
fn generate_merge_types() -> Result<String, Box<dyn std::error::Error>> {
    let output = quote! {
        use std::path::PathBuf;
        use indexmap::IndexMap;

        #[derive(Clone, Debug)]
        pub enum SettingValue {
            String(String),
            OptionString(Option<String>),
            Path(PathBuf),
            OptionPath(Option<PathBuf>),
            Bool(bool),
            Integer(i64),
            Duration(String),
        }

        pub type SourceMap = IndexMap<&'static str, SettingValue>;

        #[allow(dead_code)]
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
        pub enum SettingSource {
            Defaults,
            Config,
            Env,
            Cli,
        }

        #[allow(dead_code)]
        pub type SourceInfoMap = IndexMap<&'static str, SourceInfoEntry>;

        #[allow(dead_code)]
        #[derive(Clone, Debug)]
        pub struct SourceInfoEntry {
            pub source: SettingSource,
            pub value: String,
        }
    };

    Ok(output.to_string())
}

/// Generate metadata for introspection
fn generate_metadata(settings: &Table) -> Result<String, Box<dyn std::error::Error>> {
    let mut meta_entries = Vec::new();

    fn collect_metadata(table: &Table, prefix: &str, entries: &mut Vec<TokenStream>) {
        for (key, value) in table {
            if let Some(props) = value.as_table() {
                let full_name = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{}.{}", prefix, key)
                };

                if is_leaf_setting(props) {
                    let name_lit = full_name.as_str();
                    let typ_lit = props
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("String");
                    let default_lit = props.get("default").and_then(|v| v.as_str()).unwrap_or("");
                    let env_var = props.get("env").and_then(|v| v.as_str()).unwrap_or("");
                    let description = props
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    entries.push(quote! {
                        map.insert(
                            #name_lit,
                            SettingMeta {
                                typ: #typ_lit,
                                default_value: Some(#default_lit),
                                env_var: if #env_var.is_empty() { None } else { Some(#env_var) },
                                description: #description,
                            },
                        );
                    });
                } else {
                    // Recurse into nested groups
                    collect_metadata(props, &full_name, entries);
                }
            }
        }
    }

    collect_metadata(settings, "", &mut meta_entries);

    let output = quote! {
        use indexmap::IndexMap;
        use std::sync::LazyLock;

        #[allow(dead_code)]
        pub struct SettingMeta {
            pub typ: &'static str,
            #[allow(dead_code)]
            pub default_value: Option<&'static str>,
            pub env_var: Option<&'static str>,
            pub description: &'static str,
        }

        fn build_settings_meta() -> IndexMap<&'static str, SettingMeta> {
            let mut map = IndexMap::new();
            #(#meta_entries)*
            map
        }

        pub static SETTINGS_META: LazyLock<IndexMap<&'static str, SettingMeta>> =
            LazyLock::new(build_settings_meta);
    };

    Ok(output.to_string())
}

/// Parse type string to Rust type tokens
fn parse_type(typ: &str) -> Result<TokenStream, Box<dyn std::error::Error>> {
    Ok(match typ {
        "Bool" => quote! { bool },
        "Integer" => quote! { i64 },
        "String" => quote! { String },
        "Duration" => quote! { String }, // Stored as humantime string
        "Path" => quote! { std::path::PathBuf },
        "Url" => quote! { String },
        _ => return Err(format!("Unsupported type: {}", typ).into()),
    })
}
