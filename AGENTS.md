# MediaFireMini: istruzioni per gli agenti

## Assegnazione CI e collaborazione tra agenti

- Leggere [CI_RESOURCE_POLICY.md](CI_RESOURCE_POLICY.md) prima di modificare o avviare build cloud.
- **Provider attivo:** GitHub Actions standard: release su tag/manuale; controllo motore giornaliero.
- **Destinazione prevista:** Restare su GitHub. Il repository pubblico non consuma la quota di compute privato per i runner standard.
- L'agente prodotto gestisce sorgenti, versioni e aggiornamento del motore; l'orchestratore mantiene il provider e previene duplicazioni. Nessuna modifica della frequenza di pubblicazione implicita nella policy.
- Per manutenzione limitata a CI/documentazione eseguire controlli pertinenti ai file, senza avviare compilazioni release, installazioni o collaudi del prodotto non necessari.
- Non abilitare capacità a pagamento o cambiare provider automaticamente per quota esaurita. Indicare i controlli completati e quelli ancora da eseguire.

## Sorgenti e verifica

Applicazione desktop Rust/Slint pubblica con rilascio Windows e aggiornamento programmato del motore.

Usare i percorsi e i comandi verificati nella policy; non sostituire la struttura del progetto con un modello generico. Conservare il lavoro già presente nel checkout e dichiarare quali prove sono state realmente eseguite.
