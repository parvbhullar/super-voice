# Root Justfile — delegates to active-call-tester

# Run any tester command: just tester <command>
[no-cd]
tester +args="--list":
    cd active-call-tester && just {{args}}

# Shortcuts for common commands
test: (tester "test")
e2e: (tester "e2e-full")
smoke: (tester "e2e-smoke")
api-check: (tester "e2e-api-check")
start: (tester "start-server")
stop: (tester "stop-server")
status: (tester "server-status")
