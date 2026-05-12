# NixFleet

Declarative NixOS fleet management. Three layers, one wire protocol, no daemons on the agent path.

## What this book is

Curated guides: architecture, contracts, the operator cookbook, troubleshooting, the RFC set. Hand-written Markdown composed from the canonical sources under `docs/{design,reference,operations,rfcs}/`; this `mdbook/` tree contains only thin includes so there is one source of truth per topic.

If you are new to the codebase, read [Architecture](design/architecture.md) first, then [RFC-0003](rfcs/0003-protocol.md) for the wire protocol, then the relevant section of [Contracts](design/contracts.md) for whatever subsystem you are touching.

## Building locally

```sh
nix run .#docs        # build docs/mdbook/book/
nix run .#docs-serve  # serve + open in a browser
```

## Editing

Source files live under `docs/{design,reference,operations,rfcs}/` - the `mdbook/src/` directory contains only `{{#include}}` wrappers. Edit the canonical file in its `docs/<area>/` location, rerun `nix run .#docs`, the book picks up the change.
