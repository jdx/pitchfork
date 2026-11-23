#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

@test "a daemon template instance starts as a group and reaches its own sibling" {
  local port http_script
  port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
  http_script="$(script_path http_server.py)"
  create_pitchfork_toml <<TOML
[daemons.api]
run = "sleep 30"

[daemon_templates.stack]
instances = ["blue", "green"]

[daemon_templates.stack.daemons.api]
run = "python3 -u $http_script 0 $port"
port = $port

[daemon_templates.stack.daemons.web]
run = "echo {{ instance }} {{ instance_index }} {{ daemons.api.port }} > web_{{ instance }}; sleep 30"
depends = ["api"]
ready_delay = 0
TOML

  run pitchfork start --group stack.green
  assert_success
  wait_for_file_content web_green "green 1 $port"
  wait_for_status stack.green.api running
  wait_for_status stack.green.web running

  run pitchfork list
  assert_success
  assert_output --partial "stack.green.web"

  pitchfork stop --all
}

@test "copies of a templated port are bumped apart" {
  local port http_script
  port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
  http_script="$(script_path http_server.py)"
  create_pitchfork_toml <<TOML
[daemon_templates.stack]
instances = 2

[daemon_templates.stack.daemons.api]
run = "python3 -u $http_script 0 \$PORT"
port = { expect = [$port], bump = 20 }

[daemon_templates.stack.daemons.web]
run = "echo {{ daemons.api.port }} > web_{{ instance }}; sleep 30"
depends = ["api"]
ready_delay = 0
TOML

  run pitchfork start --group stack
  assert_success
  # Wait until both files have their content, not just exist.
  for _ in $(seq 1 100); do
    [ -s web_0 ] && [ -s web_1 ] && break
    sleep 0.1
  done
  local first second
  first="$(cat web_0)"
  second="$(cat web_1)"
  [ -n "$first" ] && [ -n "$second" ]
  [ "$first" != "$second" ]

  pitchfork stop --all
}
