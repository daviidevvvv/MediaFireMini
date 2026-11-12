# MediaFire Mini v0.3.0

Utility Windows standalone scritta in Rust con interfaccia **Slint Material 3** (`@material` 1.1.0) e motore di download **md_downloader** (`0.1.5` / `mediafire_rs` `0.2.3`) compilato direttamente nell'eseguibile.

Un solo file finale (`MediaFireMini.exe`), nessuna cartella o binario aggiuntivo richiesto.

## Funzionalità principali

- **Eseguibile unico standalone**: motore MediaFire (`md_downloader`), creazione archivi ZIP, selezione cartella nativa Windows e supporto appunti sono tutti integrati in `MediaFireMini.exe`.
- **Interfaccia Slint Material 3**: scala DPI corretta su Windows, tema chiaro/scuro automatico di sistema, pulsante rapido **Incolla** e barra di avanzamento reale (file scaricati, MiB e percentuale).
- **Selettore file integrato (`Scegli file...`)**: analizza i metadati del link MediaFire senza creare cartelle su disco e permette di filtrare e scegliere quali file scaricare prima di avviare il download.
- **Archivio ZIP opzionale**: se selezionato, crea un archivio `.zip` accanto alla cartella scaricata conservando i file originali.
- **Controllo versione `mediafire_rs` in sola lettura**: verifica l'ultima release pubblica di `NicKoehler/mediafire_rs` su GitHub e segnala eventuali nuove versioni con link alla release, senza scaricare eseguibili esterni.

## Build locale

```powershell
cargo test --all-targets --locked
cargo build --release --locked
```

L'eseguibile finale viene generato in `target\release\MediaFireMini.exe`.
