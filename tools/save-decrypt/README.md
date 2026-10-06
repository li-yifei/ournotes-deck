# Sirius save decrypt

`StarMoe-box` save files are encrypted as `magic[32] || iv[32] || Rijndael-256-CBC(key, iv, json)`. The production key/magic are injected into the Android build through `OURNOTES_SAVE_KEY` and `OURNOTES_SAVE_MAGIC`; they are not present in the APK or this repository. This directory intentionally contains no guessed decoder output. Once the operator supplies the build secrets, use the existing `SaveCodec` implementation on the Android side or port it here and emit `nnnotes.player/1` from `_player`.

Raw pulled candidates are under `resources/local/android/account/` and are retained as evidence only.
