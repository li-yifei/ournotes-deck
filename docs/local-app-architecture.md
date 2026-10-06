# Native desktop integration

The Mygo desktop application loads `https://bdon.moe/tools/deck` in the system WebView. The website manages its visual card pool and song selection. A preload adapter forwards deck Worker requests to the Go host, which invokes the Rust C ABI. Native settings handle ADB and importing a local save into the WebView's local storage.

## Computation boundary

Build the native library with `cargo build --release -p ournotes-ffi`. Outputs are `.dylib` on macOS and `.so` on Linux. JSON strings preserve exact integer IDs and fractions; release returned buffers using `ournotes_free_string`, with byte lengths from `ournotes_string_len`.

The account transport accepts `ournotes.account/1` and `ournotes-deck.recommendation-request/2`, returning `ournotes-deck.account-recommendation/1`:

- `ournotes_recommend_account_json(data_path, account_json, request_json)` computes a complete result.
- `ournotes_recommend_account_json_with_threads(..., threads)` selects the native worker allowance.
- `ournotes_search_threads_json()` reports the supported thread range and default.
- `ournotes_dataset_info_json(data_path)` and `ournotes_capabilities_json()` validate the dataset and model capabilities.

## Progress and cancellation

- `ournotes_account_job_start_json(data_path, account_json, request_json)` copies inputs and returns `{ "jobId": "..." }` immediately.
- `ournotes_account_job_poll_json(job_id)` returns `{ "done": false, "progressJson": "..." }` when a real candidate snapshot is available. Completion returns `done: true` and `resultJson`, or an `error`. Polling consumes the latest snapshot and removes completed jobs.
- `ournotes_account_job_cancel_json(job_id)` cooperatively cancels computation and removes the job. Cancellation also applies while waiting for the native search allowance.

The host retains the originating library throughout a job. The preload sends progress to the website as `{ type: "progress", jobId, inputRevision, resultJson }`, preserving the website's Worker protocol. Parallel workers aggregate genuine candidate results; temporary reports retain their unproven status.

## Local saves and optional resource tools

ADB scan output and decrypted account data stay outside version control. Store decryption credentials separately with user-only file permissions. The desktop shell imports the player save into local WebView storage; native computation receives account JSON from the page.

The optional `ournotes-adb-scan`, `ournotes-service` and `tools/resources/` utilities support standalone local-data workflows. The `ournotes.local/1` manifest resolves deck-data, assets and account paths relative to the manifest. These tools operate on resources supplied by the user. The current WebView shell obtains display resources from bdon and uses its own application storage for saves and computation caches.
