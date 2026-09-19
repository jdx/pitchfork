#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

# ============================================================================
# Slug resolution tests
# ============================================================================

@test "slug start/stop resolves daemon by slug" {
  local proj="$TEST_TEMP_DIR/slugtest"
  mkdir -p "$proj"
  cd "$proj"

  create_pitchfork_toml <<'EOF'
[daemons.api-server]
run = "sleep 60"
EOF

  run pitchfork proxy add api --daemon api-server
  assert_success

  run pitchfork start api
  assert_success

  sleep 1

  run pitchfork status api
  assert_success
  assert_output --partial "running"

  run pitchfork stop api
  assert_success

  sleep 1

  run pitchfork status api
  [[ "$output" == *"stopped"* || "$output" == *"exited"* ]]
}

@test "slug resolves from a different directory" {
  local proj="$TEST_TEMP_DIR/cross-slug"
  local other_dir="$TEST_TEMP_DIR/other"
  mkdir -p "$proj" "$other_dir"

  cd "$proj"
  create_pitchfork_toml <<'EOF'
[daemons.backend]
run = "sleep 60"
EOF

  run pitchfork proxy add be --daemon backend
  assert_success

  run pitchfork start backend
  assert_success

  sleep 1

  cd "$other_dir"
  run pitchfork status be
  assert_success
  assert_output --partial "running"

  run pitchfork stop be
  assert_success
}

@test "slug takes priority over daemon name in another namespace" {
  local proj_a="$TEST_TEMP_DIR/proj-a"
  local proj_b="$TEST_TEMP_DIR/proj-b"
  mkdir -p "$proj_a" "$proj_b"

  cd "$proj_a"
  create_pitchfork_toml <<'EOF'
[daemons.web]
run = "sleep 60"
EOF
  run pitchfork proxy add frontend --daemon web
  assert_success

  cd "$proj_b"
  create_pitchfork_toml <<'EOF'
[daemons.frontend]
run = "sleep 60"
EOF

  cd "$proj_a"
  run pitchfork start web
  assert_success
  sleep 0.5

  cd "$proj_b"
  run pitchfork start frontend
  assert_success
  sleep 1

  cd "$proj_a"
  run pitchfork status frontend
  assert_success
  assert_output --partial "running"
  [[ "$output" != *"proj-b"* ]]
  [[ "$output" == *"proj-a"* || "$output" == *"web"* ]]

  cd "$proj_a"
  run pitchfork stop web || true
  cd "$proj_b"
  run pitchfork stop frontend || true
}

@test "slug takes priority over same-named daemon in another namespace" {
  local proj_c="$TEST_TEMP_DIR/proj-c"
  local proj_d="$TEST_TEMP_DIR/proj-d"
  mkdir -p "$proj_c" "$proj_d"

  cd "$proj_c"
  create_pitchfork_toml <<'EOF'
[daemons.frontend]
run = "sleep 60"
EOF
  run pitchfork proxy add frontend-slug --daemon frontend
  assert_success

  cd "$proj_d"
  create_pitchfork_toml <<'EOF'
[daemons.frontend]
run = "sleep 60"
EOF

  cd "$proj_c"
  run pitchfork start frontend
  assert_success
  sleep 0.5

  cd "$proj_d"
  run pitchfork start frontend
  assert_success
  sleep 1

  cd "$proj_d"
  run pitchfork status frontend-slug
  assert_success
  assert_output --partial "running"
  assert_output --partial "proj-c"
  [[ "$output" != *"proj-d"* ]]

  cd "$proj_c"
  run pitchfork stop frontend || true
  cd "$proj_d"
  run pitchfork stop frontend || true
}

@test "logs command resolves daemon by slug" {
  local proj="$TEST_TEMP_DIR/slug-logs"
  mkdir -p "$proj"
  cd "$proj"

  create_pitchfork_toml <<'EOF'
[daemons.myservice]
run = "echo 'slug log test' && sleep 30"
EOF

  run pitchfork proxy add svc --daemon myservice
  assert_success

  run pitchfork start svc
  assert_success

  sleep 2

  run pitchfork logs svc -n 10
  assert_success
  assert_output --partial "slug log test"

  run pitchfork stop myservice || true
}

@test "restart command resolves daemon by slug" {
  local proj="$TEST_TEMP_DIR/slug-restart"
  mkdir -p "$proj"
  cd "$proj"

  create_pitchfork_toml <<'EOF'
[daemons.worker]
run = "sleep 60"
EOF

  run pitchfork proxy add w --daemon worker
  assert_success

  run pitchfork start w
  assert_success

  sleep 1

  run pitchfork restart w
  assert_success

  sleep 1

  run pitchfork status w
  assert_success
  assert_output --partial "running"

  run pitchfork stop worker || true
}

# ============================================================================
# Proxy URL display tests
# ============================================================================

_free_port() {
  python3 -c "import socket; s=socket.socket(); s.bind(('127.0.0.1', 0)); print(s.getsockname()[1]); s.close()"
}

@test "list shows proxy URL when proxy is enabled" {
  local proj="$TEST_TEMP_DIR/proxy-list"
  mkdir -p "$proj"
  cd "$proj"

  local port
  port=$(_free_port)

  local http_script
  http_script="$(script_path http_server.py)"

  create_pitchfork_toml <<EOF
[daemons.api]
run = "python3 -u $http_script 0 $port"
port = $port
EOF

  run pitchfork proxy add api
  assert_success

  run pitchfork start api
  assert_success
  sleep 2

  run env PITCHFORK_PROXY_ENABLE=true PITCHFORK_PROXY_TLD=localhost PITCHFORK_PROXY_PORT=7777 pitchfork list
  assert_success
  assert_output --partial "localhost:7777"

  run pitchfork stop api || true
  kill_port "$port"
}

@test "status shows proxy URL when proxy is enabled" {
  local proj="$TEST_TEMP_DIR/proxy-status"
  mkdir -p "$proj"
  cd "$proj"

  local port
  port=$(_free_port)

  local http_script
  http_script="$(script_path http_server.py)"

  create_pitchfork_toml <<EOF
[daemons.server]
run = "python3 -u $http_script 0 $port"
port = $port
EOF

  run pitchfork proxy add server
  assert_success

  run pitchfork start server
  assert_success
  sleep 2

  run env PITCHFORK_PROXY_ENABLE=true PITCHFORK_PROXY_TLD=localhost PITCHFORK_PROXY_PORT=7777 pitchfork status server
  assert_success
  assert_output --partial "Proxy:"
  assert_output --partial "localhost:7777"

  run pitchfork stop server || true
  kill_port "$port"
}

@test "start shows proxy URL when proxy is enabled" {
  local proj="$TEST_TEMP_DIR/proxy-start"
  mkdir -p "$proj"
  cd "$proj"

  local port
  port=$(_free_port)

  local http_script
  http_script="$(script_path http_server.py)"

  create_pitchfork_toml <<EOF
[daemons.app]
run = "python3 -u $http_script 0 $port"
port = $port
EOF

  run pitchfork proxy add app
  assert_success

  run env PITCHFORK_PROXY_ENABLE=true PITCHFORK_PROXY_TLD=localhost PITCHFORK_PROXY_PORT=7777 pitchfork start app
  assert_success
  [[ "$output" == *"Proxy:"* || "$output" == *"localhost:7777"* ]]

  run pitchfork stop app || true
  kill_port "$port"
}

# ============================================================================
# Proxy command tests
# ============================================================================

@test "proxy status shows disabled when proxy is off" {
  create_pitchfork_toml <<'EOF'
[daemons.dummy]
run = "sleep 1"
EOF

  run env PITCHFORK_PROXY_ENABLE=false pitchfork proxy status
  assert_success
  assert_output --partial "disabled"
}

@test "proxy status shows enabled when proxy is on" {
  create_pitchfork_toml <<'EOF'
[daemons.dummy]
run = "sleep 1"
EOF

  run env PITCHFORK_PROXY_ENABLE=true PITCHFORK_PROXY_TLD=localhost PITCHFORK_PROXY_PORT=7777 pitchfork proxy status
  assert_success
  assert_output --partial "enabled"
  assert_output --partial "Scheme:  https"
  assert_output --partial "TLD:     localhost"
  assert_output --partial "Port:    7777"
}

@test "proxy trust fails when certificate is missing" {
  create_pitchfork_toml <<'EOF'
[daemons.dummy]
run = "sleep 1"
EOF

  run pitchfork proxy trust 2>&1
  assert_failure

  assert_output --partial "CA certificate not found at"
}

# ============================================================================
# Proxy URL format tests
# ============================================================================

# A hostname is derived from a daemon's `port`, so a daemon without one is not
# routable and shows no URL, in any namespace.
@test "daemons without a port never show proxy URL regardless of namespace" {
  # Test global namespace
  local proj_dir="$TEST_TEMP_DIR/proj-global"
  mkdir -p "$proj_dir"
  cd "$proj_dir"
  create_pitchfork_toml <<EOF
[daemons.web]
run = "sleep 60"

[daemons.web.ready]
port = 0
EOF
  pitchfork supervisor start
  pitchfork start web
  run pitchfork status web
  assert_success
  [[ "$output" != *"Proxy:"* ]]

  pitchfork stop web
  pitchfork supervisor stop 2>/dev/null || true

  # Test local namespace
  local orig_state_dir="$PITCHFORK_STATE_DIR"
  local proj_local="$TEST_TEMP_DIR/proj-local"
  mkdir -p "$proj_local"
  cd "$proj_local"
  export PITCHFORK_STATE_DIR="$(mktemp -d /tmp/pf-test-XXXXXX)"
  mkdir -p "$PITCHFORK_STATE_DIR/logs"
  create_pitchfork_toml <<EOF
namespace = "myproject"

[daemons.web]
run = "sleep 60"
EOF
  pitchfork supervisor start
  pitchfork start web
  run pitchfork status web
  assert_success
  [[ "$output" != *"Proxy:"* ]]

  pitchfork stop web
  pitchfork supervisor stop 2>/dev/null || true
  rm -rf "$PITCHFORK_STATE_DIR"
  export PITCHFORK_STATE_DIR="$orig_state_dir"
}

# ============================================================================
# Header handling tests
# ============================================================================

@test "proxy rejoins split cookie header fields before forwarding" {
  local proj="$TEST_TEMP_DIR/split-cookie"
  mkdir -p "$proj"
  cd "$proj"

  local echo_script daemon_port proxy_port
  echo_script="$(to_shell_path "$(script_path cookie_echo_server.py)")"
  daemon_port=$(_free_port)
  proxy_port=$(_free_port)

  create_pitchfork_toml <<EOF
[daemons.cookie-echo]
run = 'python3 -u $echo_script $daemon_port'
port = $daemon_port
ready_http = "http://127.0.0.1:$daemon_port/"
EOF

  run pitchfork proxy add cookies --daemon cookie-echo
  assert_success

  # The supervisor owns the proxy listener, so it needs the settings in its own
  # environment. The one common_setup started predates them. `supervisor start`
  # returns once the socket accepts, and the supervisor binds the proxy before
  # it opens the socket, so no sleep is needed.
  PITCHFORK_PROXY_ENABLE=true \
    PITCHFORK_PROXY_HTTPS=false \
    PITCHFORK_PROXY_TLD=localhost \
    PITCHFORK_PROXY_PORT=$proxy_port \
    pitchfork supervisor start --force >/dev/null 2>&1

  run pitchfork start cookie-echo
  assert_success

  # An HTTP/2 client is allowed to send each cookie as its own field, and
  # browsers do. This sends the two fields over HTTP/1.1, which lands in the
  # proxy's header map in the same shape; the HTTP/2 decoding itself is
  # hyper's and is not exercised here.
  run curl -s -H "Host: cookies.localhost" \
    -H "Cookie: _session=abc123" \
    -H "Cookie: theme=dark" \
    "http://127.0.0.1:$proxy_port/"
  assert_success
  assert_output --partial "fields=1"
  assert_output --partial "cookie=_session=abc123; theme=dark"
}

# ============================================================================
# Automatic hostname tests
# ============================================================================

@test "automatic hostname routes to a daemon without a registered slug" {
  local proj="$TEST_TEMP_DIR/hostproj"
  mkdir -p "$proj"
  cd "$proj"

  local http_script daemon_port proxy_port
  http_script="$(to_shell_path "$(script_path http_server.py)")"
  daemon_port=$(_free_port)
  proxy_port=$(_free_port)

  create_pitchfork_toml <<EOF
[daemons.api]
run = 'python3 -u $http_script 0 $daemon_port'
port = $daemon_port
ready_http = "http://127.0.0.1:$daemon_port/health"
EOF

  PITCHFORK_PROXY_ENABLE=true \
    PITCHFORK_PROXY_HTTPS=false \
    PITCHFORK_PROXY_TLD=localhost \
    PITCHFORK_PROXY_PORT=$proxy_port \
    pitchfork supervisor start --force >/dev/null 2>&1

  run pitchfork start api
  assert_success

  # The hostname registry is cached for a couple of seconds, so the daemon's
  # directory may not be in it the instant the daemon starts.
  sleep 3

  # <daemon>.<project>.<tld> reaches the daemon itself.
  run curl -s -o /dev/null -w "%{http_code}" \
    -H "Host: api.hostproj.localhost" \
    "http://127.0.0.1:$proxy_port/health"
  assert_success
  assert_output "200"

  # <project>.<tld> is reserved for the project page and never routes to a daemon.
  run curl -s -w '\n%{http_code}' -H "Host: hostproj.localhost" \
    "http://127.0.0.1:$proxy_port/health"
  assert_success
  assert_output --partial "reserved"
  assert_output --partial "api.hostproj.localhost"
  [[ "${lines[-1]}" == "200" ]]

  # An unknown daemon of a known project is a 404 that lists the known names.
  run curl -s -w '\n%{http_code}' -H "Host: nope.hostproj.localhost" \
    "http://127.0.0.1:$proxy_port/health"
  assert_success
  assert_output --partial "Unknown daemon"
  assert_output --partial "api"
  [[ "${lines[-1]}" == "404" ]]

  run pitchfork stop api || true
  kill_port "$daemon_port"
}

@test "worktree hostname routes to the worktree's daemon and reaches it as PITCHFORK_URL" {
  local project="$TEST_TEMP_DIR/wtproj"
  mkdir -p "$project"
  cd "$project"
  git init -q .
  git -c user.email=t@t -c user.name=t commit -q --allow-empty -m init

  local url_script daemon_port proxy_port
  url_script="$(to_shell_path "$(script_path echo_env_server.py)")"
  daemon_port=$(_free_port)
  proxy_port=$(_free_port)

  create_pitchfork_toml <<EOF
[daemons.api]
run = 'python3 -u $url_script $daemon_port PITCHFORK_URL'
port = $daemon_port
ready_http = "http://127.0.0.1:$daemon_port/"
EOF

  git worktree add -q ../fix-login -b fix-login
  cp pitchfork.toml ../fix-login/

  PITCHFORK_PROXY_ENABLE=true \
    PITCHFORK_PROXY_HTTPS=false \
    PITCHFORK_PROXY_TLD=localhost \
    PITCHFORK_PROXY_PORT=$proxy_port \
    pitchfork supervisor start --force >/dev/null 2>&1

  # Start only the worktree's copy, so the primary checkout's daemon cannot be
  # the one answering.
  cd "$TEST_TEMP_DIR/fix-login"
  run pitchfork start api
  assert_success
  sleep 3

  # The worktree's hostname carries its label, and the daemon received that
  # same URL in its environment.
  run curl -s -H "Host: api.fix-login.wtproj.localhost" "http://127.0.0.1:$proxy_port/"
  assert_success
  assert_output --partial "http://api.fix-login.wtproj.localhost:$proxy_port"

  run pitchfork stop api || true
  kill_port "$daemon_port"
}

@test "worktree_label from a registered external config renames the hostname" {
  local project="$TEST_TEMP_DIR/labelproj"
  local generated="$TEST_TEMP_DIR/generated-label"
  mkdir -p "$project" "$generated"
  cd "$project"
  git init -q .
  git -c user.email=t@t -c user.name=t commit -q --allow-empty -m init
  create_pitchfork_toml <<'EOF'
[daemons.api]
run = "sleep 60"
port = 3999
EOF
  git worktree add -q ../wt-raw -b wt-raw
  cp pitchfork.toml ../wt-raw/

  # A generator such as mise writes its config outside the project and
  # registers it for that directory; the label must be honored from there.
  cat > "$generated/pitchfork.toml" <<'EOF'
worktree_label = "renamed"

[daemons.api]
run = "sleep 60"
port = 3999
EOF
  cd "$TEST_TEMP_DIR/wt-raw"
  run pitchfork config add "$generated/pitchfork.toml" --dir "$TEST_TEMP_DIR/wt-raw"
  assert_success

  run env PITCHFORK_PROXY_ENABLE=true PITCHFORK_PROXY_HTTPS=false \
    PITCHFORK_PROXY_TLD=localhost PITCHFORK_PROXY_PORT=7788 pitchfork proxy status
  assert_success
  assert_output --partial "api.renamed.labelproj.localhost:7788"
  refute_output --partial "api.wt-raw.labelproj.localhost"
}

@test "a slugged daemon receives the slug URL as PITCHFORK_URL" {
  local proj="$TEST_TEMP_DIR/slug-url"
  mkdir -p "$proj"
  cd "$proj"

  local env_script daemon_port proxy_port
  env_script="$(to_shell_path "$(script_path echo_env_server.py)")"
  daemon_port=$(_free_port)
  proxy_port=$(_free_port)

  create_pitchfork_toml <<EOF
[daemons.api]
run = 'python3 -u $env_script $daemon_port PITCHFORK_URL'
port = $daemon_port
ready_http = "http://127.0.0.1:$daemon_port/"
EOF

  run pitchfork proxy add legacy --daemon api
  assert_success

  PITCHFORK_PROXY_ENABLE=true \
    PITCHFORK_PROXY_HTTPS=false \
    PITCHFORK_PROXY_TLD=localhost \
    PITCHFORK_PROXY_PORT=$proxy_port \
    pitchfork supervisor start --force >/dev/null 2>&1

  run pitchfork start api
  assert_success
  sleep 2

  # The daemon is told the address the proxy actually routes: its slug, not the
  # automatic hostname it would otherwise get.
  run curl -s -H "Host: legacy.localhost" "http://127.0.0.1:$proxy_port/"
  assert_success
  assert_output --partial "http://legacy.localhost:$proxy_port"
  refute_output --partial "api.slug-url"

  run pitchfork stop api || true
  kill_port "$daemon_port"
}


# ============================================================================
# Loopback DNS resolver
# ============================================================================

# Start a supervisor with the proxy and the loopback resolver enabled.
#
# The supervisor owns both listeners, so the settings have to be in *its*
# environment; the one common_setup started predates them.
_start_proxy_with_dns() {
  local proxy_port=$1 dns_port=$2
  PITCHFORK_PROXY_ENABLE=true \
    PITCHFORK_PROXY_HTTPS=false \
    PITCHFORK_PROXY_TLD=localhost \
    PITCHFORK_PROXY_PORT="$proxy_port" \
    PITCHFORK_PROXY_DNS=true \
    PITCHFORK_PROXY_DNS_PORT="$dns_port" \
    PITCHFORK_PROXY_SYNC_HOSTS=false \
    pitchfork supervisor start --force >/dev/null 2>&1
}

@test "dns resolver answers any name under the tld, so curl --resolve is unnecessary" {
  local proj="$TEST_TEMP_DIR/dns-resolver"
  mkdir -p "$proj"
  cd "$proj"

  local daemon_port proxy_port dns_port query
  daemon_port=$(_free_port)
  proxy_port=$(_free_port)
  dns_port=$(_free_port)
  query="$(to_shell_path "$(script_path dns_query.py)")"

  create_pitchfork_toml <<EOF
[daemons.dns-web]
run = 'python3 -u -m http.server $daemon_port --bind 127.0.0.1'
port = $daemon_port
ready_http = "http://127.0.0.1:$daemon_port/"
EOF

  run pitchfork proxy add dnsweb --daemon dns-web
  assert_success

  _start_proxy_with_dns "$proxy_port" "$dns_port"

  run pitchfork start dns-web
  assert_success

  # A name nothing has registered, several labels deep: the resolver answers on
  # the shape of the name, not from a table of slugs.
  local deep="core.some-worktree.some-project.localhost"

  # `dig` is the tool a user would reach for, so prefer it when it is installed.
  if command -v dig >/dev/null 2>&1; then
    run dig +short +time=3 +tries=1 "@127.0.0.1" -p "$dns_port" "$deep" A
    assert_success
    assert_output "127.0.0.1"

    run dig +time=3 +tries=1 "@127.0.0.1" -p "$dns_port" example.com A
    assert_success
    assert_output --partial "status: REFUSED"
  fi

  run python3 "$query" "$dns_port" "$deep" A
  assert_success
  assert_output "NOERROR 127.0.0.1"

  # TCP carries the same answer, which is what a resolver falls back to.
  run python3 "$query" "$dns_port" "$deep" A --tcp
  assert_success
  assert_output "NOERROR 127.0.0.1"

  # AAAA is NODATA while the proxy listens on IPv4 only, which sends the client
  # to the A record instead of an address nothing is bound to.
  run python3 "$query" "$dns_port" "$deep" AAAA
  assert_success
  assert_output "NOERROR -"

  # Anything outside the TLD is REFUSED, so the stub resolver moves on to its
  # other servers. An authoritative NXDOMAIN would end the lookup here instead.
  run python3 "$query" "$dns_port" example.com A
  assert_success
  assert_output "REFUSED -"

  # The address the resolver hands out is the one the proxy answers on, so a
  # client that resolves through it needs no --resolve override.
  local resolved
  resolved=$(python3 "$query" "$dns_port" dnsweb.localhost A | cut -d' ' -f2)
  [ "$resolved" = "127.0.0.1" ]
  run curl -sS -o /dev/null -w '%{http_code}' \
    -H "Host: dnsweb.localhost" "http://$resolved:$proxy_port/"
  assert_success
  assert_output "200"
}

@test "proxy serves a PAC file routing the tld through the proxy" {
  local proxy_port dns_port
  proxy_port=$(_free_port)
  dns_port=$(_free_port)

  _start_proxy_with_dns "$proxy_port" "$dns_port"

  run curl -sS "http://127.0.0.1:$proxy_port/proxy.pac"
  assert_success
  assert_output --partial "function FindProxyForURL(url, host)"
  assert_output --partial "dnsDomainIs(host, \".localhost\")"
  assert_output --partial "PROXY 127.0.0.1:$proxy_port"
  assert_output --partial 'return "DIRECT";'
}

@test "proxy doctor reports the resolver and the listener" {
  local proxy_port dns_port
  proxy_port=$(_free_port)
  dns_port=$(_free_port)

  _start_proxy_with_dns "$proxy_port" "$dns_port"

  run env PITCHFORK_PROXY_ENABLE=true \
    PITCHFORK_PROXY_HTTPS=false \
    PITCHFORK_PROXY_TLD=localhost \
    PITCHFORK_PROXY_PORT="$proxy_port" \
    PITCHFORK_PROXY_DNS_PORT="$dns_port" \
    pitchfork proxy doctor
  # Not `assert_success`: whether `*.localhost` resolves through the system
  # resolver depends on the host's DNS, which this test does not configure, and
  # doctor now exits non-zero when any check fails. What it must report is that
  # the listener and the resolver are both answering.
  assert_output --partial "[ok  ] proxy listener"
  assert_output --partial "[ok  ] dns resolver"
}

@test "proxy doctor exits non-zero when a check fails" {
  # So `pitchfork proxy doctor || setup-the-proxy` works, and a CI step gating
  # on this command does not read a broken proxy as a healthy one.
  local proxy_port dns_port
  proxy_port=$(_free_port)
  dns_port=$(_free_port)

  # Nothing is started, so the listener check fails whatever the host's DNS
  # does. That makes the exit code the same everywhere this runs.
  run env PITCHFORK_PROXY_ENABLE=true \
    PITCHFORK_PROXY_HTTPS=false \
    PITCHFORK_PROXY_TLD=localhost \
    PITCHFORK_PROXY_PORT="$proxy_port" \
    PITCHFORK_PROXY_DNS_PORT="$dns_port" \
    pitchfork proxy doctor
  assert_failure
  assert_output --partial "[fail] proxy listener"
  assert_output --partial "check(s) failed"
}

@test "proxy setup prints its plan and changes nothing with --dry-run" {
  run env PITCHFORK_PROXY_ENABLE=true \
    PITCHFORK_PROXY_TLD=pftest \
    PITCHFORK_PROXY_PORT=8443 \
    pitchfork proxy setup --dry-run
  assert_success
  assert_output --partial "This will do the following:"
  # Whatever the platform decides, the plan names the TLD it is wiring up and
  # does not claim to have done anything.
  assert_output --partial "pftest"
  refute_output --partial "step(s) applied"
}

@test "proxy setup --pac plans no privileged steps" {
  run env PITCHFORK_PROXY_ENABLE=true \
    PITCHFORK_PROXY_TLD=pftest \
    PITCHFORK_PROXY_PORT=8443 \
    PITCHFORK_PROXY_HTTPS=false \
    pitchfork proxy setup --pac --dry-run
  assert_success
  assert_output --partial "/proxy.pac"
  refute_output --partial "[sudo]"
}

@test "PAC clients reach HTTPS proxy URLs through a CONNECT tunnel" {
  local proj="$TEST_TEMP_DIR/pac-connect"
  mkdir -p "$proj"
  cd "$proj"

  local daemon_port proxy_port dns_port
  daemon_port=$(_free_port)
  proxy_port=$(_free_port)
  dns_port=$(_free_port)

  create_pitchfork_toml <<EOF
[daemons.pac-web]
run = 'python3 -u -m http.server $daemon_port --bind 127.0.0.1'
port = $daemon_port
ready_http = "http://127.0.0.1:$daemon_port/"
EOF

  run pitchfork proxy add pacweb --daemon pac-web
  assert_success

  # HTTPS on, which is the configuration the PAC file is documented against:
  # a browser opening https://pacweb.localhost sends CONNECT to the proxy.
  PITCHFORK_PROXY_ENABLE=true \
    PITCHFORK_PROXY_HTTPS=true \
    PITCHFORK_PROXY_TLD=localhost \
    PITCHFORK_PROXY_PORT="$proxy_port" \
    PITCHFORK_PROXY_DNS_PORT="$dns_port" \
    PITCHFORK_PROXY_SYNC_HOSTS=false \
    PITCHFORK_PROXY_AUTO_TRUST=false \
    pitchfork supervisor start --force >/dev/null 2>&1

  run pitchfork start pac-web
  assert_success

  # The PAC file is fetchable over plain HTTP on the TLS port: the browser
  # reads it before it can trust the proxy's certificate.
  run curl -sS "http://127.0.0.1:$proxy_port/proxy.pac"
  assert_success
  assert_output --partial "PROXY 127.0.0.1:$proxy_port"

  # Exactly what a PAC-configured browser does: CONNECT, then TLS inside the
  # tunnel. --proxy-insecure is unrelated to the tunnelled certificate; -k
  # covers that, since the CA is not trusted in the test environment.
  run curl -sS -k -o /dev/null -w '%{http_code}' \
    --proxy "http://127.0.0.1:$proxy_port" \
    "https://pacweb.localhost/"
  assert_success
  assert_output "200"

  # The tunnel is not an open proxy: names outside the TLD are refused.
  run curl -sS -o /dev/null -w '%{http_code}' \
    --proxy "http://127.0.0.1:$proxy_port" \
    "https://example.com/"
  [[ "$output" == "403" || "$status" -ne 0 ]]
}

@test "the PAC path is not served for proxied hostnames, and the CA signs only the tld" {
  local proj="$TEST_TEMP_DIR/pac-host"
  mkdir -p "$proj"
  cd "$proj"

  local daemon_port proxy_port dns_port
  daemon_port=$(_free_port)
  proxy_port=$(_free_port)
  dns_port=$(_free_port)

  create_pitchfork_toml <<EOF
[daemons.pac-host-web]
run = 'python3 -u -m http.server $daemon_port --bind 127.0.0.1'
port = $daemon_port
ready_http = "http://127.0.0.1:$daemon_port/"
EOF

  run pitchfork proxy add pachost --daemon pac-host-web
  assert_success

  PITCHFORK_PROXY_ENABLE=true \
    PITCHFORK_PROXY_HTTPS=true \
    PITCHFORK_PROXY_TLD=localhost \
    PITCHFORK_PROXY_PORT="$proxy_port" \
    PITCHFORK_PROXY_DNS_PORT="$dns_port" \
    PITCHFORK_PROXY_SYNC_HOSTS=false \
    PITCHFORK_PROXY_AUTO_TRUST=false \
    pitchfork supervisor start --force >/dev/null 2>&1

  run pitchfork start pac-host-web
  assert_success

  # Addressed to the listener itself: the PAC file.
  run curl -sS "http://127.0.0.1:$proxy_port/proxy.pac"
  assert_success
  assert_output --partial "FindProxyForURL"

  # Addressed to a proxied hostname: the daemon owns that path, so this is a
  # normal request and gets the redirect to HTTPS, not the PAC file.
  run curl -sS -o /dev/null -w '%{http_code}' \
    -H "Host: pachost.localhost" "http://127.0.0.1:$proxy_port/proxy.pac"
  assert_success
  assert_output "302"

  # The same holds for every method, not just GET. Registering the route for
  # GET alone would have axum answer a POST with 405 before the fall-through
  # ran, taking the path away from the daemon for writes.
  run curl -sS -X POST -o /dev/null -w '%{http_code}' \
    -H "Host: pachost.localhost" "http://127.0.0.1:$proxy_port/proxy.pac"
  assert_success
  assert_output "302"

  # The PAC file itself is read-only, so a write aimed at the listener is
  # refused by the handler, which says what is allowed.
  run curl -sS -X POST -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:$proxy_port/proxy.pac"
  assert_success
  assert_output "405"

  run curl -sSI -X POST "http://127.0.0.1:$proxy_port/proxy.pac"
  assert_success
  assert_output --partial "GET, HEAD"

  # The TLD apex is not a routable name, so it serves the PAC file rather than
  # being treated as a proxied request that nothing can route.
  run curl -sS -H "Host: localhost" "http://127.0.0.1:$proxy_port/proxy.pac"
  assert_success
  assert_output --partial "FindProxyForURL"

  # The local CA must not mint a certificate for a name it does not serve.
  # `openssl s_client` is the closest thing to what an attacker would do.
  #
  # The subject is read back with `openssl x509` rather than matched in
  # s_client's own output, because the spacing of that line differs between
  # OpenSSL releases (`CN = x` versus `CN=x`).
  if command -v openssl >/dev/null 2>&1; then
    local subject_of="openssl x509 -noout -subject 2>/dev/null"

    # A name under the TLD gets a certificate naming it.
    run bash -c "echo | openssl s_client -connect 127.0.0.1:$proxy_port \
      -servername pachost.localhost 2>/dev/null | $subject_of"
    assert_success
    assert_output --partial "pachost.localhost"

    # A foreign name gets no certificate at all, so there is no subject to
    # read. The assertion above proves this pipeline does yield one when a
    # certificate is issued, so an empty result here is meaningful.
    run bash -c "echo | openssl s_client -connect 127.0.0.1:$proxy_port \
      -servername login.microsoftonline.com 2>/dev/null | $subject_of"
    refute_output --partial "microsoftonline"
  fi
}

@test "setup records what it did, and undo reverses it after settings change" {
  # Exercises the record end to end rather than the plan in isolation: the
  # file has to be written where undo looks for it, survive a settings change,
  # and name only what that configuration actually installed.
  # Windows has no resolver file, pf anchor or iptables rule to install, so
  # setup plans nothing but notes, reports "Nothing to change." and records
  # nothing. There is no round trip to exercise there.
  if [[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* ]]; then
    skip "setup installs nothing on Windows, so there is no record to undo"
  fi

  local first_port second_port
  first_port=$(_free_port)
  second_port=$(_free_port)

  # An unprivileged port on plain HTTP, so setup has nothing privileged to do
  # beyond the redirect, which fails harmlessly without sudo here.
  run env PITCHFORK_PROXY_ENABLE=true \
    PITCHFORK_PROXY_HTTPS=false \
    PITCHFORK_PROXY_TLD=recordtest \
    PITCHFORK_PROXY_PORT="$first_port" \
    pitchfork proxy setup --yes
  # The redirect needs root, so the run reports failure; the record is written
  # before any step runs precisely so a partial run is still reversible. That
  # only holds when setup found real work to do, so check that first: it makes
  # a platform that plans nothing say so instead of reporting a missing file.
  refute_output --partial "Nothing to change."
  assert_file_exist "$PITCHFORK_STATE_DIR/proxy/setup.toml"
  assert_file_contains "$PITCHFORK_STATE_DIR/proxy/setup.toml" "recordtest"

  # After changing the port, undo knows about both the recorded and current
  # configurations.
  run env PITCHFORK_PROXY_ENABLE=true \
    PITCHFORK_PROXY_HTTPS=false \
    PITCHFORK_PROXY_TLD=recordtest \
    PITCHFORK_PROXY_PORT="$second_port" \
    pitchfork proxy setup --undo --dry-run
  assert_success
  # Only the Linux undo names the ports, in the iptables rule it drops. The
  # macOS equivalent removes a pf anchor, whose summary names paths instead,
  # so assert on whichever this host actually plans.
  if [[ "$(uname -s)" == "Darwin" ]]; then
    assert_output --partial "pf.anchors/pitchfork"
  else
    assert_output --partial "to $first_port"
    assert_output --partial "to $second_port"
  fi

  # Undo claims a resolver file only where that configuration installed one,
  # which on Linux depends on whether systemd-resolved is running here.
  if systemctl is-active --quiet systemd-resolved 2>/dev/null; then
    assert_output --partial "resolved.conf.d"
  else
    refute_output --partial "resolved.conf.d"
  fi
}

# ============================================================================
# TLS passthrough tests
# ============================================================================

# Generate a CA, a server certificate for $1 (with a matching SAN), and a
# client certificate, all in the current directory.
_make_tls_certs() {
  local server_host="$1"
  openssl req -x509 -newkey rsa:2048 -nodes -keyout ca-key.pem -out ca.pem \
    -days 2 -subj "/CN=pitchfork-test-ca" 2>/dev/null
  openssl req -newkey rsa:2048 -nodes -keyout server-key.pem -out server.csr \
    -subj "/CN=$server_host" 2>/dev/null
  printf "subjectAltName=DNS:%s\n" "$server_host" >san.cnf
  openssl x509 -req -in server.csr -CA ca.pem -CAkey ca-key.pem -CAcreateserial \
    -out server.pem -days 2 -extfile san.cnf 2>/dev/null
  openssl req -newkey rsa:2048 -nodes -keyout client-key.pem -out client.csr \
    -subj "/CN=pitchfork-test-client" 2>/dev/null
  openssl x509 -req -in client.csr -CA ca.pem -CAkey ca-key.pem -CAcreateserial \
    -out client.pem -days 2 2>/dev/null
}

@test "passthrough splices an mTLS handshake through to the daemon" {
  skip_on_windows "openssl and python ssl paths differ under MSYS"
  if ! command -v openssl >/dev/null 2>&1; then
    skip "openssl is required to mint test certificates"
  fi

  local proj="$TEST_TEMP_DIR/passthrough-mtls"
  mkdir -p "$proj"
  cd "$proj"

  _make_tls_certs "passthru.localhost"

  local tls_script daemon_port proxy_port
  tls_script="$(to_shell_path "$(script_path mtls_echo_server.py)")"
  daemon_port=$(_free_port)
  proxy_port=$(_free_port)

  create_pitchfork_toml <<EOF
[daemons.tls-echo]
run = 'python3 -u $tls_script $daemon_port $proj/server.pem $proj/server-key.pem $proj/ca.pem'
port = $daemon_port
proxy_tls = "passthrough"
ready_port = $daemon_port
EOF

  run pitchfork proxy add passthru --daemon tls-echo
  assert_success

  # The supervisor owns the proxy listener, so the proxy settings have to be in
  # its environment. HTTPS is required: passthrough exists only on the TLS
  # listener. Auto-trust is off so the test never touches the system trust
  # store.
  PITCHFORK_PROXY_ENABLE=true \
    PITCHFORK_PROXY_HTTPS=true \
    PITCHFORK_PROXY_AUTO_TRUST=false \
    PITCHFORK_PROXY_TLD=localhost \
    PITCHFORK_PROXY_PORT=$proxy_port \
    pitchfork supervisor start --force >/dev/null 2>&1

  # The daemon is deliberately left stopped: a passthrough request must
  # auto-start it and hold the connection until it is ready, since a raw TLS
  # stream has no "Starting…" page to show.
  run curl -sS --max-time 60 \
    --cacert ca.pem --cert client.pem --key client-key.pem \
    --resolve "passthru.localhost:$proxy_port:127.0.0.1" \
    "https://passthru.localhost:$proxy_port/"
  assert_success
  # The handshake completed against the *daemon's* certificate (validated
  # against our CA, which the proxy does not hold) while presenting a client
  # certificate the daemon required. Neither is possible if the proxy
  # terminated TLS.
  assert_output --partial "client-cn=pitchfork-test-client"
  assert_output --partial "sni=passthru.localhost"

  run pitchfork stop tls-echo || true
  kill_port "$daemon_port"
}

@test "passthrough daemon reports its mode in list and status" {
  local proj="$TEST_TEMP_DIR/passthrough-mode"
  mkdir -p "$proj"
  cd "$proj"

  local port
  port=$(_free_port)

  local http_script
  http_script="$(script_path http_server.py)"

  create_pitchfork_toml <<EOF
[daemons.secure]
run = "python3 -u $http_script 0 $port"
port = $port
proxy_tls = "passthrough"
EOF

  run pitchfork proxy add secure
  assert_success

  run pitchfork start secure
  assert_success
  sleep 2

  # The mode is spelled out next to the URL in status …
  run env PITCHFORK_PROXY_ENABLE=true PITCHFORK_PROXY_TLD=localhost \
    PITCHFORK_PROXY_PORT=7777 pitchfork status secure
  assert_success
  assert_output --partial "Proxy:"
  assert_output --partial "passthrough"

  # … and annotates the URL in the list table, where only the non-default
  # mode is called out.
  run env PITCHFORK_PROXY_ENABLE=true PITCHFORK_PROXY_TLD=localhost \
    PITCHFORK_PROXY_PORT=7777 pitchfork list
  assert_success
  assert_output --partial "passthrough"

  run pitchfork stop secure || true
  kill_port "$port"
}

@test "passthrough without a port is rejected as invalid config" {
  local proj="$TEST_TEMP_DIR/passthrough-no-port"
  mkdir -p "$proj"
  cd "$proj"

  create_pitchfork_toml <<'EOF'
[daemons.broken]
run = "sleep 60"
proxy_tls = "passthrough"
EOF

  run pitchfork list
  assert_failure
  assert_output --partial "passthrough"
  assert_output --partial "port"
}

@test "passthrough hostname on a plain-HTTP proxy explains that HTTPS is required" {
  local proj="$TEST_TEMP_DIR/passthrough-http"
  mkdir -p "$proj"
  cd "$proj"

  local daemon_port proxy_port
  daemon_port=$(_free_port)
  proxy_port=$(_free_port)

  create_pitchfork_toml <<EOF
[daemons.tls-only]
run = "sleep 60"
port = $daemon_port
proxy_tls = "passthrough"
EOF

  run pitchfork proxy add tlsonly --daemon tls-only
  assert_success

  PITCHFORK_PROXY_ENABLE=true \
    PITCHFORK_PROXY_HTTPS=false \
    PITCHFORK_PROXY_TLD=localhost \
    PITCHFORK_PROXY_PORT=$proxy_port \
    pitchfork supervisor start --force >/dev/null 2>&1

  # Forwarding plain HTTP to a daemon that expects a TLS handshake would fail
  # inside the daemon, so the proxy refuses up front and says why.
  run curl -sS --max-time 20 -H "Host: tlsonly.localhost" \
    "http://127.0.0.1:$proxy_port/"
  assert_success
  assert_output --partial "passthrough"
  assert_output --partial "settings.proxy.https = true"
}


# ============================================================================
# Auto-start with dependencies
# ============================================================================

# Start a plain-HTTP proxy with auto-start. Extra `NAME=value` settings are
# passed to the supervisor, which is what reads them.
_start_autostart_proxy() {
  local proxy_port=$1
  shift
  env PITCHFORK_PROXY_ENABLE=true \
    PITCHFORK_PROXY_HTTPS=false \
    PITCHFORK_PROXY_TLD=localhost \
    PITCHFORK_PROXY_PORT="$proxy_port" \
    PITCHFORK_PROXY_AUTO_START=true \
    "$@" \
    pitchfork supervisor start --force >/dev/null 2>&1
}

# Register project directories in the global `[namespaces]` table, which is
# how the proxy knows a project before any of its daemons has run.
_register_projects() {
  local name
  : >"$PITCHFORK_CONFIG_DIR/config.toml"
  for name in "$@"; do
    cat >>"$PITCHFORK_CONFIG_DIR/config.toml" <<EOF
[namespaces.$name]
dir = "$(normalize_path "$TEST_TEMP_DIR/$name")"

EOF
  done
}

@test "requesting an app URL from a stopped state starts its dependencies first" {
  local proj="$TEST_TEMP_DIR/depproj"
  mkdir -p "$proj"
  _register_projects depproj

  local http_script app_port proxy_port order
  http_script="$(to_shell_path "$(script_path http_server.py)")"
  app_port=$(_free_port)
  proxy_port=$(_free_port)
  order="$(to_shell_path "$proj/order.log")"

  # db takes a moment to become ready, so a dependent started early would
  # record itself first.
  cat >"$proj/pitchfork.toml" <<EOF
[daemons.db]
run = "sleep 1 && echo db >> '$order' && echo 'db ready' && sleep 60"
ready_output = "db ready"

[daemons.migrate]
run = "echo migrate >> '$order'"
oneshot = true
depends = ["db"]

[daemons.app]
run = "echo app >> '$order' && python3 -u $http_script 0 $app_port"
port = $app_port
ready_http = "http://127.0.0.1:$app_port/health"
depends = ["migrate"]
EOF

  _start_autostart_proxy "$proxy_port"

  # The project page is read-only: opening it starts nothing.
  run curl -s -o /dev/null -w "%{http_code}" --max-time 20 \
    -H "Host: depproj.localhost" "http://127.0.0.1:$proxy_port/"
  assert_success
  assert_output "200"
  sleep 1
  assert_file_not_exist "$proj/order.log"
  run pitchfork status depproj/db
  refute_output --partial "running"

  # The first request waits for the whole graph and is then served.
  run curl -s -w '\n%{http_code}' --max-time 60 \
    -H "Host: app.depproj.localhost" "http://127.0.0.1:$proxy_port/health"
  assert_success
  assert_line --index 0 "OK"
  assert_line --index 1 "200"

  run cat "$proj/order.log"
  assert_output "$(printf 'db\nmigrate\napp')"

  run pitchfork status depproj/migrate
  assert_output --partial "completed"
  run pitchfork status depproj/db
  assert_output --partial "running"

  pitchfork stop --all || true
  kill_port "$app_port"
}

@test "auto-start resolves a dependency in another registered project" {
  mkdir -p "$TEST_TEMP_DIR/shared" "$TEST_TEMP_DIR/web"
  _register_projects shared web

  local http_script app_port proxy_port order
  http_script="$(to_shell_path "$(script_path http_server.py)")"
  app_port=$(_free_port)
  proxy_port=$(_free_port)
  order="$(to_shell_path "$TEST_TEMP_DIR/order.log")"

  cat >"$TEST_TEMP_DIR/shared/pitchfork.toml" <<EOF
[daemons.db]
run = "sleep 1 && echo db >> '$order' && echo 'db ready' && sleep 60"
ready_output = "db ready"
EOF
  cat >"$TEST_TEMP_DIR/web/pitchfork.toml" <<EOF
[daemons.app]
run = "echo app >> '$order' && python3 -u $http_script 0 $app_port"
port = $app_port
ready_http = "http://127.0.0.1:$app_port/health"
depends = ["shared/db"]
EOF

  _start_autostart_proxy "$proxy_port"

  run curl -s -w '\n%{http_code}' --max-time 60 \
    -H "Host: app.web.localhost" "http://127.0.0.1:$proxy_port/health"
  assert_success
  assert_line --index 1 "200"

  run cat "$TEST_TEMP_DIR/order.log"
  assert_output "$(printf 'db\napp')"
  run pitchfork status shared/db
  assert_output --partial "running"

  pitchfork stop --all || true
  kill_port "$app_port"
}

@test "auto-start reports a failed dependency and leaves the app stopped" {
  local proj="$TEST_TEMP_DIR/failproj"
  mkdir -p "$proj"
  _register_projects failproj

  local app_port proxy_port order
  app_port=$(_free_port)
  proxy_port=$(_free_port)
  order="$(to_shell_path "$proj/order.log")"

  cat >"$proj/pitchfork.toml" <<EOF
[daemons.migrate]
run = "echo migrate >> '$order' && exit 3"
oneshot = true

[daemons.app]
run = "echo app >> '$order' && sleep 60"
port = $app_port
depends = ["migrate"]
EOF

  _start_autostart_proxy "$proxy_port"

  run curl -s -w '\n%{http_code}' --max-time 60 \
    -H "Host: app.failproj.localhost" "http://127.0.0.1:$proxy_port/"
  assert_success
  assert_output --partial "dependency 'failproj/migrate' failed"
  assert_line --index -1 "502"

  run cat "$proj/order.log"
  assert_output "migrate"
  run pitchfork status failproj/app
  refute_output --partial "running"
}

@test "concurrent auto-starts sharing a dependency wait for it to be ready" {
  local proj="$TEST_TEMP_DIR/sharedeps"
  mkdir -p "$proj"
  _register_projects sharedeps

  local http_script port1 port2 proxy_port order marker
  http_script="$(to_shell_path "$(script_path http_server.py)")"
  port1=$(_free_port)
  port2=$(_free_port)
  proxy_port=$(_free_port)
  order="$(to_shell_path "$proj/order.log")"
  marker="$(to_shell_path "$proj/db.ready")"

  # Each app refuses to start unless db has finished starting, so an app
  # started against a db that is only "running" fails.
  cat >"$proj/pitchfork.toml" <<EOF
[daemons.db]
run = "echo db >> '$order' && sleep 2 && touch '$marker' && echo 'db ready' && sleep 60"
ready_output = "db ready"

[daemons.one]
run = "test -f '$marker' && echo one >> '$order' && python3 -u $http_script 0 $port1"
port = $port1
ready_http = "http://127.0.0.1:$port1/health"
depends = ["db"]

[daemons.two]
run = "test -f '$marker' && echo two >> '$order' && python3 -u $http_script 0 $port2"
port = $port2
ready_http = "http://127.0.0.1:$port2/health"
depends = ["db"]
EOF

  _start_autostart_proxy "$proxy_port"

  curl -s -o "$TEST_TEMP_DIR/one.out" -w '%{http_code}' --max-time 60 \
    -H "Host: one.sharedeps.localhost" "http://127.0.0.1:$proxy_port/health" \
    >"$TEST_TEMP_DIR/one.code" &
  local pid1=$!
  curl -s -o "$TEST_TEMP_DIR/two.out" -w '%{http_code}' --max-time 60 \
    -H "Host: two.sharedeps.localhost" "http://127.0.0.1:$proxy_port/health" \
    >"$TEST_TEMP_DIR/two.code" &
  local pid2=$!
  wait "$pid1" "$pid2"

  run cat "$TEST_TEMP_DIR/one.code"
  assert_output "200"
  run cat "$TEST_TEMP_DIR/two.code"
  assert_output "200"

  # db started once, before either app.
  run grep -c '^db$' "$proj/order.log"
  assert_output "1"
  run head -n 1 "$proj/order.log"
  assert_output "db"

  pitchfork stop --all || true
  kill_port "$port1"
  kill_port "$port2"
}

@test "an auto-start that outlives its request keeps starting the graph" {
  local proj="$TEST_TEMP_DIR/slowdeps"
  mkdir -p "$proj"
  _register_projects slowdeps

  local http_script app_port proxy_port order
  http_script="$(to_shell_path "$(script_path http_server.py)")"
  app_port=$(_free_port)
  proxy_port=$(_free_port)
  order="$(to_shell_path "$proj/order.log")"

  cat >"$proj/pitchfork.toml" <<EOF
[daemons.db]
run = "sleep 4 && echo db >> '$order' && echo 'db ready' && sleep 60"
ready_output = "db ready"

[daemons.app]
run = "echo app >> '$order' && python3 -u $http_script 0 $app_port"
port = $app_port
ready_http = "http://127.0.0.1:$app_port/health"
depends = ["db"]
EOF

  _start_autostart_proxy "$proxy_port" PITCHFORK_PROXY_AUTO_START_TIMEOUT=1s

  run curl -s -w '\n%{http_code}' --max-time 30 \
    -H "Host: app.slowdeps.localhost" "http://127.0.0.1:$proxy_port/health"
  assert_success
  assert_output --partial "timed out"
  assert_output --partial "continues in the background"

  # No further request is made: the graph finishes on its own, in order.
  wait_for_status slowdeps/app running
  run cat "$proj/order.log"
  assert_output "$(printf 'db\napp')"

  pitchfork stop --all || true
  kill_port "$app_port"
}
