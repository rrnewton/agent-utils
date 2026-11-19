.PHONY: contract
# THE WIRE CONTRACT. `src/contract.rs` names the JSON the server and the page exchange; the files
# under contract/ and web/contract.* are generated from it and checked in, so a change to a wire
# type arrives as a reviewable diff. `contract` regenerates all of them: the schema and samples
# from the Rust types, then the browser's validators and declarations from the schema, then every
# contract test against the result. The browser-validator test is skipped while rewriting: it
# would judge the OLD validators against the NEW samples, and stop the recipe before regenerating.
# `contract-check` fails, naming the stale file, when the browser files no longer match the
# schema; `cargo test --test integration contract::` does the same for the schema against the Rust types.
#
# Both, and `typecheck`, need the generator's pinned tools: `npm ci --prefix contract` once.
# Serving the page never does -- web/contract.js is plain checked-in JavaScript.
contract:
	VIBE_TALK_UPDATE_CONTRACT=1 cargo test --locked --test integration -- contract:: --skip the_browser_validators
	node contract/generate.mjs
	cargo test --locked --test integration contract::
