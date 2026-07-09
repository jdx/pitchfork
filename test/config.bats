#!/usr/bin/env bats

setup() {
  load test_helper/common_setup
  _common_setup
}

teardown() {
  _common_teardown
}

# Helper function to read pitchfork.toml
read_toml() {
  cat pitchfork.toml
}

@test "config add with positional arguments creates correct toml" {
  run pitchfork daemons add api bun run server/index.ts
  assert_success
  assert_output --partial "added "
  assert_output --partial "/api to "
  assert [ -f pitchfork.toml ]
  
  run read_toml
  assert_output --partial 'run = "bun run server/index.ts"'
}

@test "config add with all options combined" {
  run pitchfork daemons add api \
    --run "bun run server/index.ts" \
    --retry 3 \
    --watch "server/**/*.ts" \
    --watch "package.json" \
    --dir "./api" \
    --env "NODE_ENV=development" \
    --env "PORT=3000" \
    --ready-delay 3 \
    --ready-output "Listening on" \
    --depends database \
    --autostart \
    --autostop \
    --on-ready "echo 'API is ready'"
  assert_success
  
  run read_toml
  assert_output --partial 'run = "bun run server/index.ts"'
  assert_output --partial 'retry = 3'
  assert_output --partial 'watch = ["server/**/*.ts", "package.json"]'
  assert_output --partial 'dir = "./api"'
  assert_output --partial 'NODE_ENV = "development"'
  assert_output --partial 'PORT = "3000"'
  assert_output --partial 'ready_delay = 3'
  assert_output --partial 'ready_output = "Listening on"'
  assert_output --partial 'depends = ["database"]'
  assert_output --partial "auto = ["
  assert_output --partial "[daemons.api.hooks]"
  refute_output --partial 'run = "--'
}

@test "config add fails without run command" {
  run pitchfork daemons add api
  assert_failure
  assert_output --partial "--run" || assert_output --partial "arguments" || assert_output --partial "required"
}

@test "config add preserves existing daemons" {
  # First add a daemon
  run pitchfork daemons add postgres --run "postgres -D data"
  assert_success
  
  # Then add another daemon
  run pitchfork daemons add api --run "npm start"
  assert_success
  
  run read_toml
  # Both daemons should exist
  assert_output --partial "[daemons.postgres]"
  assert_output --partial "[daemons.api]"
  assert_output --partial 'run = "postgres -D data"'
  assert_output --partial 'run = "npm start"'
}

@test "config add generates valid config that can start a daemon" {
  # Create a simple script that outputs "ready"
  cat > server.sh <<'EOF'
#!/bin/bash
echo 'ready'
sleep 30
EOF
  chmod +x server.sh
  
  # Add the daemon using config add
  run pitchfork daemons add test-server --run "./server.sh" --ready-output "ready" --retry 0
  assert_success
  
  # Try to start the daemon
  run pitchfork start test-server
  assert_success
  
  # If start fails, ensure it's not due to config parsing
  refute_output --partial "invalid option"
  refute_output --partial "usage: exec"
  refute_output --partial "parse error"
  
  # Verify the daemon actually reaches running status
  wait_for_status test-server running
  
  # Clean up
  run pitchfork stop test-server || true
}
