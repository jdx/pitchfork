use crate::Result;
use crate::pitchfork_toml::PitchforkToml;
use schemars::schema_for;

/// Generate JSON Schema for pitchfork.toml configuration
#[derive(Debug, clap::Args)]
#[clap(hide = true)]
pub struct Schema;

impl Schema {
    pub async fn run(&self) -> Result<()> {
        let schema = schema_for!(PitchforkToml);
        let json = serde_json::to_string_pretty(&schema).unwrap();
        println!("{json}");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_all_config_sections() {
        let schema = serde_json::to_value(schema_for!(PitchforkToml)).unwrap();
        let properties = schema["properties"].as_object().unwrap();

        for section in [
            "daemons",
            "env",
            "groups",
            "namespace",
            "namespaces",
            "settings",
            "slugs",
        ] {
            assert!(
                properties.contains_key(section),
                "schema is missing the `{section}` config section"
            );
        }
        assert!(!properties.contains_key("path"));
        assert!(
            schema["required"]
                .as_array()
                .is_none_or(|required| required.is_empty()),
            "top-level config sections must remain optional"
        );
        assert_eq!(
            schema["$defs"]["GroupEntryRaw"]["properties"]["daemons"]["items"]["$ref"],
            "#/$defs/DaemonId",
            "group members must use the daemon ID schema"
        );
    }
}
