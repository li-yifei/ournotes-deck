# Local save extraction

Desktop applications can pull Our Notes saves from `/sdcard/Android/data/<package>/files/<account>/` using an authorized ADB connection. Save containers use `magic[32] || iv[32] || Rijndael-256-CBC(key, iv, json)`; keys and magic values are supplied through local `OURNOTES_SAVE_KEY` and `OURNOTES_SAVE_MAGIC` configuration.

After decryption, preserve the original `_player` JSON bytes and integer IDs. The account adapter accepts an `ournotes.account/1` envelope. Keep encrypted saves, decrypted players, credentials, master tables and extracted assets in ignored local storage. Generated extraction status is local to each run.
