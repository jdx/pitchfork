//! Tera template rendering for pitchfork.toml configuration fields.
//!
//! Allows `run`, `env` values, `hooks.*`, and the readiness fields (`ready_cmd`,
//! `ready_http`, `ready_port`, `ready_output`) to use Tera templates like
//! `{{ daemons.redis.ports[0] }}` to reference computed values from other daemons.
//!
//! Templates are resolved level-by-level along the dependency order: each level
//! can reference daemons from previous levels (which have already started and
//! had their ports resolved).

use crate::daemon_id::DaemonId;
use crate::pitchfork_toml::PitchforkTomlDaemon;
use crate::settings::settings;
use indexmap::IndexMap;
use std::collections::HashMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// DaemonTemplateState
// ---------------------------------------------------------------------------

/// Resolved state of a daemon available for template rendering.
#[derive(Debug, Clone)]
pub struct DaemonTemplateState {
    pub ports: Vec<u16>,
    pub id: String,
    pub name: String,
    pub namespace: String,
    pub slug: Option<String>,
    pub dir: PathBuf,
}

impl DaemonTemplateState {
    fn port(&self) -> Option<u16> {
        self.ports.first().copied()
    }
}

// ---------------------------------------------------------------------------
// TemplateContext
// ---------------------------------------------------------------------------

/// Context for rendering Tera templates in pitchfork.toml fields.
pub struct TemplateContext {
    self_state: DaemonTemplateState,
    daemon_states: HashMap<String, DaemonTemplateState>,
}

impl TemplateContext {
    /// Build a template context for a daemon.
    ///
    /// - `id`: the daemon being rendered
    /// - `daemon_config`: its pitchfork.toml config
    /// - `resolved_daemons`: map of daemon ID -> resolved ports from previous levels
    /// - `daemon_configs`: the full PitchforkToml.daemons map for looking up dir/slug
    pub fn new(
        id: &DaemonId,
        daemon_config: &PitchforkTomlDaemon,
        resolved_daemons: &HashMap<DaemonId, Vec<u16>>,
        daemon_configs: &IndexMap<DaemonId, PitchforkTomlDaemon>,
    ) -> Self {
        let global_slugs = crate::pitchfork_toml::PitchforkToml::read_global_slugs();
        let dir = crate::ipc::batch::resolve_daemon_dir(
            daemon_config.dir.as_deref(),
            daemon_config.path.as_deref(),
        );

        let self_state = DaemonTemplateState {
            ports: Vec::new(),
            id: id.qualified(),
            name: id.name().to_string(),
            namespace: id.namespace().to_string(),
            slug: crate::pitchfork_toml::PitchforkToml::find_slug_for_daemon_in_registry(
                id,
                &global_slugs,
            ),
            dir,
        };

        let mut daemon_states = HashMap::new();
        for (dep_id, ports) in resolved_daemons {
            if let Some(config) = daemon_configs.get(dep_id) {
                let dep_dir = crate::ipc::batch::resolve_daemon_dir(
                    config.dir.as_deref(),
                    config.path.as_deref(),
                );
                let state = DaemonTemplateState {
                    ports: ports.clone(),
                    id: dep_id.qualified(),
                    name: dep_id.name().to_string(),
                    namespace: dep_id.namespace().to_string(),
                    slug: crate::pitchfork_toml::PitchforkToml::find_slug_for_daemon_in_registry(
                        dep_id,
                        &global_slugs,
                    ),
                    dir: dep_dir,
                };

                // Short names are only valid within the current namespace.
                if dep_id.namespace() == id.namespace() {
                    daemon_states.insert(dep_id.name().to_string(), state.clone());
                }

                // Register with qualified key (namespace.name) for all namespaces.
                daemon_states.insert(qualified_key(dep_id), state);
            }
        }

        Self {
            self_state,
            daemon_states,
        }
    }

    /// Convert this context into a Tera Context for rendering.
    pub fn to_tera_context(&self) -> tera::Context {
        let mut ctx = tera::Context::new();

        // Self variables
        ctx.insert("name", &self.self_state.name);
        ctx.insert("namespace", &self.self_state.namespace);
        ctx.insert("id", &self.self_state.id);
        ctx.insert("slug", &self.self_state.slug);
        ctx.insert("dir", &self.self_state.dir.to_string_lossy().to_string());

        // Daemons
        let mut daemons_map = serde_json::Map::new();
        for (name, state) in &self.daemon_states {
            if daemons_map.contains_key(name) {
                continue;
            }
            daemons_map.insert(name.clone(), daemon_state_to_json(state));
        }
        ctx.insert("daemons", &serde_json::Value::Object(daemons_map));

        // Settings
        let s = settings();
        ctx.insert(
            "settings",
            &serde_json::json!({
                "proxy": {
                    "enable": s.proxy.enable,
                    "tld": s.proxy.tld,
                    "port": s.proxy.port,
                    "https": s.proxy.https,
                }
            }),
        );

        // Always expose proxy_url so templates can distinguish an unroutable daemon
        // via a strict null value instead of an undefined-variable error.
        let proxy_url = build_proxy_url(self.self_state.slug.as_deref(), &s);
        ctx.insert("proxy_url", &proxy_url);

        ctx
    }
}

fn daemon_state_to_json(state: &DaemonTemplateState) -> serde_json::Value {
    serde_json::json!({
        "port": state.port(),
        "ports": state.ports,
        "id": state.id,
        "name": state.name,
        "namespace": state.namespace,
        "slug": state.slug,
        "dir": state.dir.to_string_lossy(),
    })
}

/// Convert a DaemonId into a template key using `namespace.name` format.
/// E.g. `myproj/redis` -> `myproj.redis`
fn qualified_key(id: &DaemonId) -> String {
    format!("{}.{}", id.namespace(), id.name())
}

/// Build a proxy URL from slug and settings.
fn build_proxy_url(slug: Option<&str>, s: &crate::settings::Settings) -> Option<String> {
    let slug = slug?;
    let scheme = if s.proxy.https { "https" } else { "http" };
    let tld = &s.proxy.tld;
    let standard_port = if s.proxy.https { 443u16 } else { 80u16 };
    let effective_port = u16::try_from(s.proxy.port).ok().filter(|&p| p > 0)?;
    let host = format!("{slug}.{tld}");
    Some(if effective_port == standard_port {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{effective_port}")
    })
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render a Tera template string with the given context.
///
/// Returns the rendered string, or an error describing what went wrong.
/// Fast path: strings without `{{` or `{%` are returned as-is.
pub fn render_template(template: &str, context: &TemplateContext) -> Result<String, RenderError> {
    TemplateRenderer::new(context).render(template)
}

/// Render all template-enabled fields of a daemon config.
///
/// Modifies the config in place. Returns the first error encountered from
/// non-hook fields (`run`, `env`, `ready_*`). Hook template errors are
/// logged as warnings and the hook is set to `None` — hooks are re-rendered
/// at fire time via `fire_hook`, so pre-rendered hook strings are unused.
pub fn render_daemon_templates(
    config: &mut PitchforkTomlDaemon,
    context: &TemplateContext,
) -> Result<(), RenderError> {
    let mut renderer = TemplateRenderer::new(context);

    config.run = renderer.render(&config.run)?;

    if let Some(ref env) = config.env {
        let rendered: IndexMap<String, String> = env
            .iter()
            .map(|(k, v)| Ok((k.clone(), renderer.render(v)?)))
            .collect::<Result<_, RenderError>>()?;
        config.env = Some(rendered);
    }

    if let Some(ref hooks) = config.hooks {
        let rendered = crate::config_types::PitchforkTomlHooks {
            on_ready: hooks
                .on_ready
                .as_deref()
                .and_then(|t| renderer.render(t).ok()),
            on_fail: hooks
                .on_fail
                .as_deref()
                .and_then(|t| renderer.render(t).ok()),
            on_retry: hooks
                .on_retry
                .as_deref()
                .and_then(|t| renderer.render(t).ok()),
            on_stop: hooks
                .on_stop
                .as_deref()
                .and_then(|t| renderer.render(t).ok()),
            on_exit: hooks
                .on_exit
                .as_deref()
                .and_then(|t| renderer.render(t).ok()),
            on_output: hooks.on_output.as_ref().and_then(|hook| {
                renderer
                    .render(&hook.run)
                    .ok()
                    .map(|run| crate::config_types::OnOutputHook {
                        run,
                        filter: hook.filter.clone(),
                        regex: hook.regex.clone(),
                        debounce: hook.debounce.clone(),
                    })
            }),
        };
        config.hooks = Some(rendered);
    }

    if let Some(ref cmd) = config.ready_cmd {
        config.ready_cmd = Some(renderer.render(cmd)?);
    }

    if let Some(ref output) = config.ready_output {
        config.ready_output = Some(renderer.render(output)?);
    }

    if let Some(ref http) = config.ready_http {
        let mut http = http.clone();
        http.url = renderer.render(&http.url)?;
        config.ready_http = Some(http);
    }

    if let Some(crate::config_types::ReadyPort::Template(ref template)) = config.ready_port {
        let rendered = renderer.render(template)?;
        let port = rendered
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|&p| p > 0)
            .ok_or_else(|| RenderError::InvalidPort {
                template: template.clone(),
                rendered: rendered.clone(),
            })?;
        config.ready_port = Some(crate::config_types::ReadyPort::Port(port));
    }

    Ok(())
}

fn contains_template_syntax(template: &str) -> bool {
    template.contains("{{") || template.contains("{%") || template.contains("{#")
}

struct TemplateRenderer {
    tera: tera::Tera,
    context: tera::Context,
    next_template_id: usize,
}

impl TemplateRenderer {
    fn new(context: &TemplateContext) -> Self {
        let mut tera = tera::Tera::default();
        tera.register_filter(
            "default",
            |value: &tera::Value,
             kwargs: tera::Kwargs,
             _: &tera::State|
             -> tera::TeraResult<tera::Value> {
                let default_val = kwargs.must_get::<tera::Value>("value")?;
                let boolean = kwargs.get::<bool>("boolean")?.unwrap_or_default();
                if value.is_undefined() || value.is_none() || (boolean && !value.is_truthy()) {
                    Ok(default_val)
                } else {
                    Ok(value.clone())
                }
            },
        );
        Self {
            tera,
            context: context.to_tera_context(),
            next_template_id: 0,
        }
    }

    fn render(&mut self, template: &str) -> Result<String, RenderError> {
        if !contains_template_syntax(template) {
            return Ok(template.to_string());
        }

        let template_name = format!("config_{}", self.next_template_id);
        self.next_template_id += 1;

        self.tera
            .add_raw_template(&template_name, template)
            .map_err(|e| RenderError::TemplateSyntax {
                template: template.to_string(),
                source: e,
            })?;

        self.tera
            .render(&template_name, &self.context)
            .map_err(|e| RenderError::RenderFailed {
                template: template.to_string(),
                source: e,
            })
    }
}

// ---------------------------------------------------------------------------
// RenderError
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("template syntax error in {template:?}: {source}")]
    TemplateSyntax {
        template: String,
        source: tera::Error,
    },
    #[error("template render failed for {template:?}: {source}")]
    RenderFailed {
        template: String,
        source: tera::Error,
    },
    #[error(
        "ready_port template {template:?} rendered to {rendered:?}, expected a port number (1-65535)"
    )]
    InvalidPort { template: String, rendered: String },
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_daemon_config(run: &str) -> PitchforkTomlDaemon {
        PitchforkTomlDaemon {
            run: run.to_string(),
            ..Default::default()
        }
    }

    fn make_context_with_daemon(name: &str, ports: Vec<u16>) -> TemplateContext {
        let id = DaemonId::new("myproj", name);
        let config = make_daemon_config("echo");
        let mut resolved = HashMap::new();
        resolved.insert(id.clone(), ports);
        let mut configs = IndexMap::new();
        configs.insert(id.clone(), make_daemon_config("echo"));
        TemplateContext::new(
            &DaemonId::new("myproj", "self"),
            &config,
            &resolved,
            &configs,
        )
    }

    #[test]
    fn test_no_template_passthrough() {
        let ctx = make_context_with_daemon("redis", vec![6379]);
        assert_eq!(render_template("hello world", &ctx).unwrap(), "hello world");
    }

    #[test]
    fn test_self_variables() {
        let id = DaemonId::new("myproj", "api");
        let config = make_daemon_config("echo");
        let ctx = TemplateContext::new(&id, &config, &HashMap::new(), &IndexMap::new());

        assert_eq!(render_template("{{ name }}", &ctx).unwrap(), "api");
        assert_eq!(render_template("{{ namespace }}", &ctx).unwrap(), "myproj");
        assert_eq!(render_template("{{ id }}", &ctx).unwrap(), "myproj/api");
    }

    #[test]
    fn test_daemon_port_reference() {
        let ctx = make_context_with_daemon("redis", vec![6379]);
        assert_eq!(
            render_template("{{ daemons.redis.port }}", &ctx).unwrap(),
            "6379"
        );
    }

    #[test]
    fn test_daemon_ports_array() {
        let ctx = make_context_with_daemon("redis", vec![6379, 6380]);
        assert_eq!(
            render_template("{{ daemons.redis.ports[0] }}", &ctx).unwrap(),
            "6379"
        );
        assert_eq!(
            render_template("{{ daemons.redis.ports[1] }}", &ctx).unwrap(),
            "6380"
        );
    }

    #[test]
    fn test_daemon_qualified_name() {
        let ctx = make_context_with_daemon("redis", vec![6379]);
        assert_eq!(
            render_template("{{ daemons[\"myproj.redis\"].port }}", &ctx).unwrap(),
            "6379"
        );
    }

    #[test]
    fn test_short_name_only_matches_current_namespace() {
        let self_id = DaemonId::new("app", "api");
        let self_config = make_daemon_config("echo");
        let other_id = DaemonId::new("infra", "redis");

        let mut resolved = HashMap::new();
        resolved.insert(other_id.clone(), vec![6379]);

        let mut configs = IndexMap::new();
        configs.insert(other_id.clone(), make_daemon_config("echo"));

        let ctx = TemplateContext::new(&self_id, &self_config, &resolved, &configs);

        assert!(render_template("{{ daemons.redis.port }}", &ctx).is_err());
        assert_eq!(
            render_template("{{ daemons[\"infra.redis\"].port }}", &ctx).unwrap(),
            "6379"
        );
    }

    #[test]
    fn test_settings_reference() {
        let ctx = make_context_with_daemon("redis", vec![6379]);
        let result = render_template("{{ settings.proxy.tld }}", &ctx).unwrap();
        // Default TLD is "localhost"
        assert_eq!(result, "localhost");
    }

    #[test]
    fn test_undefined_variable_error() {
        let ctx = make_context_with_daemon("redis", vec![6379]);
        let result = render_template("{{ nonexistent }}", &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_comment_only_template_is_parsed() {
        let ctx = make_context_with_daemon("redis", vec![6379]);
        assert_eq!(
            render_template("before{# hidden #}after", &ctx).unwrap(),
            "beforeafter"
        );
    }

    #[test]
    fn test_proxy_url_is_present_as_null_when_slug_is_missing() {
        let id = DaemonId::new("myproj", "api");
        let config = make_daemon_config("echo");
        let ctx = TemplateContext::new(&id, &config, &HashMap::new(), &IndexMap::new());

        assert_eq!(
            render_template("{{ proxy_url | default(value=\"none\") }}", &ctx).unwrap(),
            "none"
        );
    }

    #[test]
    fn test_mixed_template_and_literal() {
        let ctx = make_context_with_daemon("redis", vec![6379]);
        assert_eq!(
            render_template("redis://localhost:{{ daemons.redis.port }}/0", &ctx).unwrap(),
            "redis://localhost:6379/0"
        );
    }

    #[test]
    fn test_render_daemon_templates_run() {
        let ctx = make_context_with_daemon("redis", vec![6379]);
        let mut config = PitchforkTomlDaemon {
            run: "redis-cli -p {{ daemons.redis.port }}".to_string(),
            ..Default::default()
        };
        render_daemon_templates(&mut config, &ctx).unwrap();
        assert_eq!(config.run, "redis-cli -p 6379");
    }

    #[test]
    fn test_render_daemon_templates_env() {
        let ctx = make_context_with_daemon("redis", vec![6379]);
        let mut config = PitchforkTomlDaemon {
            run: "echo".to_string(),
            env: Some(IndexMap::from([
                (
                    "DATABASE_URL".to_string(),
                    "redis://localhost:{{ daemons.redis.port }}/0".to_string(),
                ),
                ("STATIC_VAR".to_string(), "unchanged".to_string()),
            ])),
            ..Default::default()
        };
        render_daemon_templates(&mut config, &ctx).unwrap();
        let env = config.env.unwrap();
        assert_eq!(env["DATABASE_URL"], "redis://localhost:6379/0");
        assert_eq!(env["STATIC_VAR"], "unchanged");
    }

    #[test]
    fn test_render_daemon_templates_on_output_run() {
        let ctx = make_context_with_daemon("redis", vec![6379]);
        let mut config = PitchforkTomlDaemon {
            run: "echo".to_string(),
            hooks: Some(crate::config_types::PitchforkTomlHooks {
                on_ready: None,
                on_fail: None,
                on_retry: None,
                on_stop: None,
                on_exit: None,
                on_output: Some(crate::config_types::OnOutputHook {
                    run: "curl http://localhost:{{ daemons.redis.port }}".to_string(),
                    filter: Some("ready".to_string()),
                    regex: None,
                    debounce: None,
                }),
            }),
            ..Default::default()
        };

        render_daemon_templates(&mut config, &ctx).unwrap();

        let hooks = config.hooks.unwrap();
        let on_output = hooks.on_output.unwrap();
        assert_eq!(on_output.run, "curl http://localhost:6379");
        assert_eq!(on_output.filter.as_deref(), Some("ready"));
    }

    #[test]
    fn test_render_daemon_templates_ready_fields() {
        use crate::config_types::{ReadyHttp, ReadyPort};

        let ctx = make_context_with_daemon("redis", vec![6379]);
        let mut config = PitchforkTomlDaemon {
            run: "echo".to_string(),
            ready_cmd: Some("redis-cli -p {{ daemons.redis.port }} ping".to_string()),
            ready_output: Some("listening on {{ daemons.redis.port }}".to_string()),
            ready_http: Some(ReadyHttp {
                url: "http://localhost:{{ daemons.redis.port }}/health".to_string(),
                status: vec![200, 401],
            }),
            ready_port: Some(ReadyPort::Template("{{ daemons.redis.port }}".to_string())),
            ..Default::default()
        };

        render_daemon_templates(&mut config, &ctx).unwrap();

        assert_eq!(config.ready_cmd.as_deref(), Some("redis-cli -p 6379 ping"));
        assert_eq!(config.ready_output.as_deref(), Some("listening on 6379"));
        let http = config.ready_http.unwrap();
        assert_eq!(http.url, "http://localhost:6379/health");
        assert_eq!(http.status, vec![200, 401]);
        assert_eq!(config.ready_port, Some(ReadyPort::Port(6379)));
    }

    #[test]
    fn test_render_daemon_templates_ready_port_invalid() {
        use crate::config_types::ReadyPort;

        let ctx = make_context_with_daemon("redis", vec![6379]);
        let mut config = PitchforkTomlDaemon {
            run: "echo".to_string(),
            ready_port: Some(ReadyPort::Template("{{ name }}".to_string())),
            ..Default::default()
        };

        let err = render_daemon_templates(&mut config, &ctx).unwrap_err();
        assert!(matches!(err, RenderError::InvalidPort { .. }));
    }

    #[test]
    fn test_render_daemon_templates_ready_port_literal_untouched() {
        use crate::config_types::ReadyPort;

        let ctx = make_context_with_daemon("redis", vec![6379]);
        let mut config = PitchforkTomlDaemon {
            run: "echo".to_string(),
            ready_port: Some(ReadyPort::Port(8080)),
            ..Default::default()
        };

        render_daemon_templates(&mut config, &ctx).unwrap();
        assert_eq!(config.ready_port, Some(ReadyPort::Port(8080)));
    }

    #[test]
    fn test_render_daemon_templates_hook_error_does_not_fail() {
        let ctx = make_context_with_daemon("redis", vec![6379]);
        let mut config = PitchforkTomlDaemon {
            run: "echo".to_string(),
            hooks: Some(crate::config_types::PitchforkTomlHooks {
                on_ready: Some("{{ nonexistent }}".to_string()),
                on_fail: None,
                on_retry: None,
                on_stop: None,
                on_exit: None,
                on_output: None,
            }),
            ..Default::default()
        };

        // Hook template errors are silently converted to None — daemon still starts
        render_daemon_templates(&mut config, &ctx).unwrap();
        let hooks = config.hooks.unwrap();
        assert!(hooks.on_ready.is_none());
    }

    #[test]
    fn test_render_daemon_templates_run_error_still_fails() {
        let ctx = make_context_with_daemon("redis", vec![6379]);
        let mut config = PitchforkTomlDaemon {
            run: "{{ nonexistent }}".to_string(),
            ..Default::default()
        };

        // Non-hook template errors still propagate as Err
        assert!(render_daemon_templates(&mut config, &ctx).is_err());
    }
}
