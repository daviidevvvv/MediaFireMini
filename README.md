# MediaFire Mini v0.2.0

Utility Windows con interfaccia nativa Win32 scritta in Rust. Usa MDRS (mediafire_rs) per scaricare cartelle MediaFire e creare automaticamente uno ZIP conservando la struttura originale.

## Utilizzo

1. Apri MediaFire Mini dal collegamento sul Desktop.
2. Incolla il link HTTPS di MediaFire.
3. Scegli la cartella di destinazione.
4. Se vuoi un archivio ZIP, lascia selezionata la casella.
5. Premi Scarica cartella, poi Apri destinazione.

Ogni download occupa una cartella nuova, senza sovrascrivere file preesistenti. Lo ZIP viene creato a fianco e i file originali rimangono.

## Novita v0.2.0 - aggiornamenti da GitHub

All'avvio l'app controlla in background la release piu recente del progetto https://github.com/NicKoehler/mediafire_rs/releases/latest e confronta la versione con quella di mdrs.exe installato.

Se trova una nuova release stabile, mostra un avviso e abilita il pulsante Aggiorna. Un clic scarica l'asset ufficiale mdrs-windows-x64.zip, verifica dimensione e SHA256 pubblicato da GitHub, estrae mdrs.exe, conserva il vecchio binario in mdrs.exe.bak, sostituisce il motore e verifica la versione. In caso di errore ripristina il vecchio binario.

Il controllo all'avvio non scarica eseguibili. Serve un clic esplicito per aggiornare. Non richiede un account GitHub.
Il pulsante Verifica permette di ripetere il controllo in qualunque momento.
Sono considerate solo le release ufficiali latest, non commit o nightly.

## Aggiornamento della GUI - pronto ma repository ancora da pubblicare

La GUI ha un meccanismo per aggiornarsi da un proprio repository GitHub pubblico con GitHub Releases. Questo progetto oggi e' solo locale: NON e' stato pubblicato automaticamente.

Quando esistera una repository pubblica autorizzata:
- crea un file gui-update-repo.txt accanto a MediaFireMini.exe;
- scrivi OWNER/REPO nella prima riga;
- pubblica nuove release stabili con tag v0.2.1, v0.3.0 ecc.;
- in ogni release pubblica MediaFireMini.exe come asset con digest SHA256 GitHub.

A quel punto il programma segnala la nuova release e il pulsante Aggiorna puo aggiornare anche la GUI. MediaFireMiniUpdater.exe aggiorna la GUI dopo la chiusura e riavvia il programma, conservando la versione precedente in un .bak.

Un repository privato non e' accessibile con l'API pubblica senza credenziali. Non salvare token in gui-update-repo.txt e non rendere pubblici repository senza consenso.

## File portatili

- MediaFireMini.exe - GUI Win32 Rust
- MediaFireMiniUpdater.exe - helper per autoaggiornamento GUI
- mdrs.exe - motore MDRS originale
- README.md - queste istruzioni

Sorgente locale: C:\Users\david\Documents\MediaFireMini
Test: cargo test --all-targets
Build: cargo build --release --bins

## Sicurezza, test e limiti

- Check GitHub su HTTPS, solo repository configurati.
- SHA256 e dimensioni obbligatori prima di sostituire eseguibili.
- Copia di rollback del motore e della GUI.
- Non aggiorna mentre scarica.
- Non usa cookie del browser e non esegue installer Lecram.
- Non modifica Oblivion Remastered.
- Test reale dell'aggiornamento MDRS superato in cartella temporanea: 0.2.2 -> 0.2.3.
- Non esiste ancora una release GitHub della GUI, quindi il suo autoaggiornamento futuro rimane predisposto ma non attivato.
