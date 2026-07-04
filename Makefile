# WVM build pipeline:
#   1. build the app for wasm32-wasip2
#   2. compose it with the vendored SQLite component (satisfies the sqlite import)
#   3. build the native bootstrapper, embedding the composed app

APP_WASM      := target/wasm32-wasip2/release/wvm_app.wasm
COMPOSED      := target/wvm-app.composed.wasm
SQLITE        := vendor/sqlite-core.wasm

.PHONY: all app compose native clean ci act

all: native

app:
	cargo build -p wvm-app --target wasm32-wasip2 --release

compose: app
	wac plug $(APP_WASM) --plug $(SQLITE) -o $(COMPOSED)

native: compose
	WVM_APP_WASM=$(CURDIR)/$(COMPOSED) cargo build -p wvm --release

clean:
	cargo clean

# Run the same checks CI runs, locally and without Docker.
ci: all
	cargo fmt --all --check
	cargo clippy -p wvm-core -p wvm --release -- -D warnings
	cargo clippy -p wvm-app --target wasm32-wasip2 --release -- -D warnings
	cargo test

# Run the CI workflow in Docker via nektos/act (uses .actrc). Resolve the active
# Docker context's socket so this works on Colima (whose socket is not at the
# default /var/run/docker.sock) without the caller exporting DOCKER_HOST.
# The visibility gate (if: !repository.private) is truthy under act, so the job
# runs locally even while the GitHub repo is private.
act:
	DOCKER_HOST="$$(docker context inspect --format '{{.Endpoints.docker.Host}}' 2>/dev/null || echo unix:///var/run/docker.sock)" \
		act -W .github/workflows/ci.yml
