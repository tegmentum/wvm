# WVM build pipeline:
#   1. build the app for wasm32-wasip2
#   2. compose it with the vendored SQLite component (satisfies the sqlite import)
#   3. build the native bootstrapper, embedding the composed app

APP_WASM      := target/wasm32-wasip2/release/wvm_app.wasm
COMPOSED      := target/wvm-app.composed.wasm
SQLITE        := vendor/sqlite-core.wasm

.PHONY: all app compose native clean

all: native

app:
	cargo build -p wvm-app --target wasm32-wasip2 --release

compose: app
	wac plug $(APP_WASM) --plug $(SQLITE) -o $(COMPOSED)

native: compose
	WVM_APP_WASM=$(CURDIR)/$(COMPOSED) cargo build -p wvm --release

clean:
	cargo clean
