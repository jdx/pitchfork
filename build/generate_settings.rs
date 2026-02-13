// Code generator for settings from settings.toml
// Based on the pattern from hk (https://github.com/jdx/hk)
//
// This module generates type-safe Rust structs from a settings.toml schema file.
// It creates two files in OUT_DIR/generated/:
// - settings.rs: Main Settings struct with nested sub-structs, Default impls,
//   Partial structs, merge/apply helpers, env-var loading, and duration helpers
// - settings_meta.rs: Metadata for introspection (env vars, defaults, etc.)

use heck::ToUpperCamelCase;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::fs;
use std::path::PathBuf;
use toml::Table;

/// Header prepended to every generated file for readability.
const GENERATED_FILE_HEADER: &str = "// @generated — do not edit by hand.\n\n";

pub fn generate() -> Result<(), Box<dyn std::error::Error>> {
    let settings_toml = fs::read_to_string("settings.toml")?;
    let settings: Table = settings_toml.parse()?;

    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    let generated_dir = out_dir.join("generated");
    fs::create_dir_all(&generated_dir)?;

    // Generate settings.rs - the main Settings struct and nested structs
    let settings_rs = format!(
        "{}{}",
        GENERATED_FILE_HEADER,
        generate_settings_struct(&settings)?
    );
    fs::write(generated_dir.join("settings.rs"), settings_rs)?;

    // Generate settings_meta.rs - metadata for introspection
    let settings_meta_rs = format!("{}{}", GENERATED_FILE_HEADER, generate_metadata(&settings)?);
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

    // Generate all Partial structs (SettingsGeneralPartial, etc., and SettingsPartial)
    tokens.extend(generate_partial_struct_and_nested("Settings", settings)?);

    // Generate SettingsPartial::merge_from
    let partial_merge_body = generate_partial_merge_from_body(settings, "self", "other");
    tokens.extend(quote! {
        impl SettingsPartial {
            /// Merge another partial onto this one.
            /// All `Some` values in `other` override the corresponding values in `self`.
            pub fn merge_from(&mut self, other: &Self) {
                #partial_merge_body
            }
        }
    });

    // Generate Settings impl with load methods and env var loading
    let load_env = generate_load_from_env(settings, "self");
    let duration_helpers = generate_duration_helpers(settings);
    let apply_partial_body = generate_apply_partial_body(settings, "self", "partial");

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
                        match std::fs::read_to_string(&path) {
                            Err(e) => eprintln!("pitchfork: warning: failed to read {}: {}", path.display(), e),
                            Ok(content) => match content.parse::<toml::Table>() {
                                Err(e) => eprintln!("pitchfork: warning: failed to parse {}: {}", path.display(), e),
                                Ok(table) => {
                                    if let Some(settings_table) = table.get("settings") {
                                        match settings_table.clone().try_into::<SettingsPartial>() {
                                            Err(e) => eprintln!("pitchfork: warning: invalid [settings] in {}: {}", path.display(), e),
                                            Ok(partial) => settings.apply_partial(&partial),
                                        }
                                    }
                                }
                            },
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

            /// Apply a partial settings overlay.
            /// Only `Some` values in `partial` will override the corresponding fields in `self`.
            pub fn apply_partial(&mut self, partial: &SettingsPartial) {
                #apply_partial_body
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
                // Child struct name is derived from the current struct name to handle
                // arbitrary nesting depth: Settings + General + Foo → SettingsGeneralFoo
                let child_struct_name = format!("{}{}", struct_name, key.to_upper_camel_case());
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
                // Must match the naming convention in generate_struct
                let child_struct_name = format!("{}{}", struct_name, key.to_upper_camel_case());
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
            // The value from settings.toml is already the plain string content.
            // Do not strip quotes here: if the author wrote default = "hello",
            // TOML delivers the string hello (without quotes) to us.
            quote! { #default.to_string() }
        }
        "Path" => {
            // Handle complex expressions like dirs::config_dir().
            // Only treat the value as a raw Rust expression when it explicitly
            // uses the `dirs::` namespace; any other string (including paths
            // that happen to contain "::" such as Windows paths or URLs) is
            // passed through as a plain string literal.
            if default.starts_with("dirs::") {
                let tokens: TokenStream = default.parse()?;
                quote! { #tokens }
            } else {
                quote! { std::path::PathBuf::from(#default) }
            }
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

                    // Check for deprecated env var (must be a different name from env)
                    let deprecated_env = props
                        .get("deprecated_env")
                        .and_then(|v| v.as_str())
                        .filter(|dep| *dep != env_var);

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
                        "String" => quote! {
                            if let Ok(val) = std::env::var(#env_var) {
                                #field_path = val;
                            }
                        },
                        "Duration" => {
                            // Main env var: validate as humantime, warn on invalid
                            let main_block = quote! {
                                if let Ok(val) = std::env::var(#env_var) {
                                    if humantime::parse_duration(&val).is_ok() {
                                        #field_path = val;
                                    } else {
                                        eprintln!(
                                            "pitchfork: warning: invalid duration {:?} for {}, using default",
                                            val, #env_var
                                        );
                                    }
                                }
                            };

                            // Deprecated env var (different name): fallback when the new
                            // env var is not set. Try humantime first, then bare integer
                            // (interpreted as seconds for backward compat).
                            let deprecated_block = if let Some(dep_env) = deprecated_env {
                                quote! {
                                    if std::env::var(#env_var).is_err() {
                                        if let Ok(val) = std::env::var(#dep_env) {
                                            eprintln!(
                                                "pitchfork: warning: {} is deprecated, use {} instead (with duration format, e.g. \"10s\", \"1m\")",
                                                #dep_env, #env_var
                                            );
                                            if humantime::parse_duration(&val).is_ok() {
                                                #field_path = val;
                                            } else if let Ok(n) = val.parse::<u64>() {
                                                // Best-effort: treat bare integer as seconds
                                                #field_path = format!("{}s", n);
                                            } else {
                                                eprintln!(
                                                    "pitchfork: warning: invalid value {:?} for {}, using default",
                                                    val, #dep_env
                                                );
                                            }
                                        }
                                    }
                                }
                            } else {
                                quote! {}
                            };

                            quote! {
                                #main_block
                                #deprecated_block
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

/// Generate `{struct_name}Partial` struct and all nested `*Partial` structs (depth-first).
/// `struct_name` is the NON-partial name (e.g., "Settings", "SettingsGeneral").
fn generate_partial_struct_and_nested(
    struct_name: &str,
    table: &Table,
) -> Result<TokenStream, Box<dyn std::error::Error>> {
    let mut tokens = TokenStream::new();

    // Recurse into nested groups first (depth-first, so children are defined before parents)
    for (key, value) in table {
        if let Some(child_table) = value.as_table()
            && !is_leaf_setting(child_table)
        {
            // Child struct name follows the same convention as generate_struct:
            // parent struct name + child key in UpperCamelCase
            let child_struct_name = format!("{}{}", struct_name, key.to_upper_camel_case());
            tokens.extend(generate_partial_struct_and_nested(
                &child_struct_name,
                child_table,
            )?);
        }
    }

    // Generate THIS level's partial struct: `{struct_name}Partial`
    let partial_struct_name = format!("{}Partial", struct_name);
    let partial_ident = format_ident!("{}", partial_struct_name);
    let mut fields = Vec::new();

    for (key, value) in table {
        let field_ident = format_ident!("{}", key);
        if let Some(props) = value.as_table() {
            if is_leaf_setting(props) {
                let type_str = props
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("String");
                let rust_type = parse_type(type_str)?;
                let doc = props
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                fields.push(quote! {
                    #[doc = #doc]
                    #[serde(skip_serializing_if = "Option::is_none", default)]
                    pub #field_ident: Option<#rust_type>
                });
            } else {
                // Nested group: non-Option field, defaults to all-None partial
                let child_struct_name = format!("{}{}", struct_name, key.to_upper_camel_case());
                let child_partial_ident = format_ident!("{}Partial", child_struct_name);
                fields.push(quote! {
                    #[serde(default)]
                    pub #field_ident: #child_partial_ident
                });
            }
        }
    }

    tokens.extend(quote! {
        #[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
        #[serde(default)]
        pub struct #partial_ident {
            #(#fields),*
        }
    });

    Ok(tokens)
}

/// Generate the body of `SettingsPartial::merge_from`.
/// For each `Some` field in `other`, overrides the corresponding field in `self`.
fn generate_partial_merge_from_body(
    table: &Table,
    self_path: &str,
    other_path: &str,
) -> TokenStream {
    let mut stmts = Vec::new();

    for (key, value) in table {
        if let Some(props) = value.as_table() {
            if is_leaf_setting(props) {
                let self_field: TokenStream = format!("{}.{}", self_path, key).parse().unwrap();
                let other_field: TokenStream = format!("{}.{}", other_path, key).parse().unwrap();
                stmts.push(quote! {
                    if #other_field.is_some() {
                        #self_field = #other_field.clone();
                    }
                });
            } else {
                // Nested group: recurse
                let nested = generate_partial_merge_from_body(
                    props,
                    &format!("{}.{}", self_path, key),
                    &format!("{}.{}", other_path, key),
                );
                stmts.push(nested);
            }
        }
    }

    quote! { #(#stmts)* }
}

/// Generate the body of `Settings::apply_partial`.
/// For each `Some` field in `partial`, overrides the corresponding field in `self`.
fn generate_apply_partial_body(table: &Table, self_path: &str, partial_path: &str) -> TokenStream {
    let mut stmts = Vec::new();

    for (key, value) in table {
        if let Some(props) = value.as_table() {
            if is_leaf_setting(props) {
                let self_field: TokenStream = format!("{}.{}", self_path, key).parse().unwrap();
                let partial_field: TokenStream =
                    format!("{}.{}", partial_path, key).parse().unwrap();
                let type_str = props
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("String");
                // Bool and Integer are Copy; String/Duration/Path need clone
                let stmt = match type_str {
                    "Bool" | "Integer" => quote! {
                        if let Some(v) = #partial_field {
                            #self_field = v;
                        }
                    },
                    _ => quote! {
                        if let Some(ref v) = #partial_field {
                            #self_field = v.clone();
                        }
                    },
                };
                stmts.push(stmt);
            } else {
                // Nested group: recurse
                let nested = generate_apply_partial_body(
                    props,
                    &format!("{}.{}", self_path, key),
                    &format!("{}.{}", partial_path, key),
                );
                stmts.push(nested);
            }
        }
    }

    quote! { #(#stmts)* }
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

                        let setting_name = if prefix.is_empty() {
                            key.to_string()
                        } else {
                            format!("{}.{}", prefix, key)
                        };

                        methods.push(quote! {
                            #[doc = #doc_comment]
                            #[allow(dead_code)]
                            pub fn #method_name(&self) -> std::time::Duration {
                                Self::parse_duration(&#field_path)
                                    .unwrap_or_else(|| {
                                        eprintln!(
                                            "pitchfork: warning: invalid duration {:?} for setting {}, using default {:?}",
                                            #field_path, #setting_name, #default_duration
                                        );
                                        #fallback
                                    })
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
                    // Emit None when no default key exists, not Some("") which is misleading.
                    let default_value_tokens = match props.get("default").and_then(|v| v.as_str()) {
                        Some(s) => quote! { Some(#s) },
                        None => quote! { None },
                    };
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
                                default_value: #default_value_tokens,
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
        _ => return Err(format!("Unsupported type: {}", typ).into()),
    })
}
