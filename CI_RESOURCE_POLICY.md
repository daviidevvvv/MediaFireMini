# CI e assegnazione dei lavori: MediaFireMini

Verifica iniziale: 23 novembre 2026. Questa policy descrive assegnazioni e stato effettivo; non abilita da sola alcun servizio.

## Profilo e provider

Applicazione desktop Rust/Slint pubblica con rilascio Windows e aggiornamento programmato del motore.

- **Attivo:** GitHub Actions standard: release su tag/manuale; controllo motore giornaliero.
- **Assegnazione prevista:** Restare su GitHub. Il repository pubblico non consuma la quota di compute privato per i runner standard.

## Comandi e confini

- `cargo test --all-targets --locked`.
- `cargo build --release --locked`; risultato `target/release/MediaFireMini.exe`.
- Per il rilascio riusare `.github/workflows/release.yml`; non avviarlo per documenti o istruzioni agenti.

## Decisioni operative

- Il workflow `auto_engine_update.yml` controlla il motore e costruisce/pubblica solo quando cambia; preservarne il comportamento.
- Un job pubblico gratuito usa comunque runner, storage e tempo: evitare dispatch doppi sullo stesso tag.
- Non spostare queste build in Azure: sottrarrebbero capacità privata agli altri progetti senza risolvere un problema di quota GitHub privata.

## Ripartizione tra agenti

L'agente prodotto gestisce sorgenti, versioni e aggiornamento del motore; l'orchestratore mantiene il provider e previene duplicazioni. Nessuna modifica della frequenza di pubblicazione implicita nella policy.

## Regole condivise

- GitHub rimane la fonte del codice. Le quote private sono condivise per account GitHub e per organizzazione Azure DevOps, non per repository.
- Assegnare ogni build pesante a un solo provider per commit. Usare cache e artefatti già validi; evitare rilanci equivalenti e compilazioni release per documentazione.
- Non abilitare runner a pagamento, aumento di budget o fallback automatico verso un altro provider. Se una quota è esaurita, completare i controlli locali disponibili e riferire ciò che resta da eseguire.
- Le modifiche alla sola CI/policy richiedono le verifiche pertinenti a quei file; le suite di prodotto restano obbligatorie quando il codice interessato cambia.
- Prima di modificare file usati da un altro agente leggere stato e diff correnti; preservare il suo lavoro e rendere esplicita la parte di cui si assume la responsabilità.

## Prossimo passaggio

Mantenere la CI pubblica attuale; valutare cache e serializzazione dei rilasci solo se emerge un problema concreto di durata o concorrenza.
