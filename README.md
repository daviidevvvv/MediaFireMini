# MediaFire Mini v0.3.4

Utility Windows standalone scritta in Rust con interfaccia **Slint Material 3** (`@material` 1.1.0) e motore di download **md_downloader** (`0.1.5` / `mediafire_rs` `0.2.3`) compilato direttamente nell'eseguibile.

Un solo file finale (`MediaFireMini.exe`), nessuna cartella o binario aggiuntivo richiesto.

## Funzionalità principali

- **Eseguibile unico standalone**: motore MediaFire (`md_downloader`), creazione archivi ZIP, selezione cartella nativa Windows e supporto appunti sono tutti integrati in `MediaFireMini.exe`.
- **Interfaccia Slint Material 3**: scala DPI corretta su Windows, tema chiaro/scuro automatico di sistema e barra di avanzamento con file scaricati, MiB e percentuale. Il pulsante **Incolla** e il campo del link aggiornano la cache locale (`%LOCALAPPDATA%\MediaFireMini\cache.json`), che ripropone l'ultimo link MediaFire valido alla riapertura.
- **Selettore file integrato (`Scegli file...`)**: naviga tra le sottocartelle e seleziona singoli file o intere cartelle. Il filtro cerca per nome e percorso, l'ordine A–Z o Z–A mostra prima le selezioni e **Solo selezionati** raccoglie i file scelti in tutte le cartelle.
- **Archivio ZIP opzionale**: se selezionato, crea un archivio `.zip` accanto alla cartella scaricata conservando i file originali.
- **Auto-update a file singolo**: controlla automaticamente le release di `daviidevvvv/MediaFireMini` su GitHub, scarica il nuovo `MediaFireMini.exe` e ne verifica lo SHA256. L'app prepara la sostituzione accanto all'eseguibile, lo riavvia e rimuove la copia precedente. Se il riavvio fallisce, prova a ripristinare la versione precedente e mostra l'errore. Un workflow GitHub Actions giornaliero aggiorna automaticamente `md_downloader`/`md_api` quando escono nuove versioni.

## Aggiornamento dalla v0.3.1 o v0.3.2

Se usi la v0.3.1 o la v0.3.2, chiudi l'app e sostituisci una sola volta il vecchio eseguibile con la [versione più recente](https://github.com/daviidevvvv/MediaFireMini/releases/latest). Dalla v0.3.3 puoi usare **Aggiorna ora**.

## Build locale

```powershell
cargo test --all-targets --locked
cargo build --release --locked
```

L'eseguibile finale viene generato in `target\release\MediaFireMini.exe`.
