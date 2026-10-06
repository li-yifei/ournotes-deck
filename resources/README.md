# Local resources

The desktop app reads private resources from a user-selected directory. The
repository keeps this directory empty: account saves, deck data and game assets
come from the user's own local export or an Android cache extraction.

Stage files with:

```sh
python3 tools/resources/stage.py \
  --output resources/local \
  --deck-data /path/to/deck-data.json \
  --roster /path/to/roster.json \
  --assets /path/to/extracted/assets
```

The command writes `resources/local/manifest.json` with SHA-256 hashes and stable
relative paths:

```text
resources/local/
├── manifest.json
├── nnnotes.deck-data/1
├── account/roster.json
└── assets/…
```

The MyGo shell should expose the selected `resources/local` path as its resource
root. Rust receives the manifest-resolved deck data and account paths through the
FFI/API layer. Assets remain ordinary files so the WebView can load them through a
local file or app URL handler.

Resource contents are intentionally excluded from version control. Keep the
manifest with the staged files so the app can detect stale or mismatched data
before running a recommendation.
