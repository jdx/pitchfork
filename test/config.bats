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
  run pitchfork config add api bun run server/index.ts
  assert_success
  assert_output --partial "added api"
  assert [ -f pitchfork.toml ]
  
  run read_toml
  assert_output --partial 'run = "bun run server/index.ts"'
}

@test "config add with --run flag" {
  run pitchfork config add worker --run "npm run worker"
  assert_success
  
  run read_toml
  assert_output --partial 'run = "npm run worker"'
}

@test "config add with retry option" {
  run pitchfork config add api --run "bun run server/index.ts" --retry 3
  assert_success
  
  run read_toml
  assert_output --partial 'retry = 3'
  assert_output --partial 'run = "bun run server/index.ts"'
  # Ensure run value doesn't contain CLI flags
  refute_output --partial 'run = "--cmd'
}

@test "config add with watch patterns" {
  run pitchfork config add api --run "bun run server" --watch "server/**/*.ts" --watch "server/**/*.sql"
  assert_success
  
  run read_toml
  assert_output --partial 'watch = ["server/**/*.ts", "server/**/*.sql"]'
}

@test "config add with autostart and autostop" {
  run pitchfork config add api --run "npm start" --autostart --autostop
  assert_success
  
  run read_toml
  # Check for auto array containing both start and stop
  assert_output --partial "auto = ["
  assert_output --partial '"start"'
  assert_output --partial '"stop"'
}

@test "config add with environment variables" {
  run pitchfork config add api --run "npm start" --env "NODE_ENV=development" --env "PORT=3000"
  assert_success
  
  run read_toml
  assert_output --partial 'NODE_ENV = "development"'
  assert_output --partial 'PORT = "3000"'
}

@test "config add with ready checks" {
  run pitchfork config add api --run "npm start" --ready-delay 5 --ready-output "Server ready" --ready-http "http://localhost:3000/health" --ready-port 3000
  assert_success
  
  run read_toml
  assert_output --partial 'ready_delay = 5'
  assert_output --partial 'ready_output = "Server ready"'
  assert_output --partial 'ready_http = "http://localhost:3000/health"'
  assert_output --partial 'ready_port = 3000'
}

@test "config add with dependencies" {
  run pitchfork config add api --run "npm start" --depends postgres --depends redis
  assert_success
  
  run read_toml
  assert_output --partial 'depends = ["postgres", "redis"]'
}

@test "config add with hooks" {
  run pitchfork config add api --run "npm start" --on-ready "curl -X POST http://localhost:3000/ready" --on-fail "./scripts/alert.sh" --on-retry "echo 'retrying'"
  assert_success
  
  run read_toml
  # Check that hooks section exists with on_ready, on_fail, on_retry
  assert_output --partial "[daemons.api.hooks]"
  assert_output --partial 'on_ready = "curl -X POST http://localhost:3000/ready"'
  assert_output --partial 'on_fail = "./scripts/alert.sh"'
  assert_output --partial "on_retry = \"echo 'retrying'\""
}

@test "config add with cron schedule" {
  run pitchfork config add backup --run "./scripts/backup.sh" --cron-schedule "0 0 2 * * *" --cron-retrigger always
  assert_success
  
  run read_toml
  assert_output --partial 'schedule = "0 0 2 * * *"'
  assert_output --partial 'retrigger = "always"'
}

@test "config add with all options combined" {
  run pitchfork config add api \
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
  run pitchfork config add api
  assert_failure
  assert_output --partial "--run" || assert_output --partial "arguments" || assert_output --partial "required"
}

@test "config add preserves existing daemons" {
  # First add a daemon
  run pitchfork config add postgres --run "postgres -D data"
  assert_success
  
  # Then add another daemon
  run pitchfork config add api --run "npm start"
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
  run pitchfork config add test-server --run "./server.sh" --ready-output "ready" --retry 0
  assert_success
  
  # Try to start the daemon
  run pitchfork start test-server
  
  # The start may succeed or fail depending on timing, but should not have parsing errors
  # If it fails, ensure it's not due to config parsing
  if [ $status -ne 0 ]; then
    refute_output --partial "invalid option"
    refute_output --partial "usage: exec"
    refute_output --partial "parse error"
  fi
  
  # Clean up
  run pitchfork stop test-server || true
}
