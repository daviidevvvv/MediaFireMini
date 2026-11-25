# MediaFire Mini v0.3.3

Utility Windows standalone scritta in Rust con interfaccia **Slint Material 3** (`@material` 1.1.0) e motore di download **md_downloader** (`0.1.5` / `mediafire_rs` `0.2.3`) compilato direttamente nell'eseguibile.

Un solo file finale (`MediaFireMini.exe`), nessuna cartella o binario aggiuntivo richiesto.

## Funzionalità principali

- **Eseguibile unico standalone**: motore MediaFire (`md_downloader`), creazione archivi ZIP, selezione cartella nativa Windows e supporto appunti sono tutti integrati in `MediaFireMini.exe`.
- **Interfaccia Slint Material 3**: scala DPI corretta su Windows, tema chiaro/scuro automatico di sistema, pulsante rapido **Incolla**, cache locale dell'ultimo link incollato (`%LOCALAPPDATA%\MediaFireMini\cache.json`) e barra di avanzamento reale (file scaricati, MiB e percentuale).
- **Selettore file integrato (`Scegli file...`)**: analizza i metadati del link MediaFire senza creare cartelle su disco e permette di filtrare, selezionare/deselezionare tutto e ordinare per nome (A-Z / Z-A) i file prima di avviare il download.
- **Archivio ZIP opzionale**: se selezionato, crea un archivio `.zip` accanto alla cartella scaricata conservando i file originali.
- **Auto-update a file singolo**: controlla automaticamente le release di `daviidevvvv/MediaFireMini` su GitHub, scarica il nuovo `MediaFireMini.exe` e ne verifica lo SHA256. L'app prepara la sostituzione accanto all'eseguibile, lo riavvia e rimuove la copia precedente. Se il riavvio fallisce, prova a ripristinare la versione precedente e mostra l'errore. Un workflow GitHub Actions giornaliero aggiorna automaticamente `md_downloader`/`md_api` quando escono nuove versioni.

## Aggiornamento dalla v0.3.1 o v0.3.2

Per passare dalla v0.3.1 o v0.3.2 alla v0.3.3, chiudi l'app e sostituisci una sola volta il vecchio eseguibile con [MediaFireMini.exe v0.3.3](https://github.com/daviidevvvv/MediaFireMini/releases/tag/v0.3.3). Le versioni precedenti contengono il programma di aggiornamento difettoso.

## Build locale

```powershell
cargo test --all-targets --locked
cargo build --release --locked
```

L'eseguibile finale viene generato in `target\release\MediaFireMini.exe`.
