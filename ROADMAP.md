# TV Tray — evoluzioni possibili

Idee rimandate dopo la versione 0.2.0. Nessuna è implementata; l'ordine non è una priorità.

## Modulo SSH (Developer Mode)

Collegamento SSH alla TV (porta 9922, utente `prisoner`) con la passphrase mostrata dall'app
*Developer Mode* sulla TV. La passphrase si inserisce una sola volta nelle Impostazioni e viene
conservata cifrata con DPAPI, come la chiave SSAP. Il key server è sulla porta 9991 e risponde
solo con l'app Developer Mode aperta sulla TV.

È la base delle voci successive.

## Rinnovo automatico della sessione Developer Mode

Evitare di riaprire a mano l'app *Developer Mode* sulla TV quando la sessione sta per scadere.

- Leggere via SSH `/var/luna/preferences/devmode_enabled` per ottenere il `sessionToken`.
- Rinnovare con `https://developer.lge.com/secure/ResetDevModeSession.dev?sessionToken=<token>`.
- Leggere la scadenza con `https://developer.lge.com/secure/CheckDevModeSession.dev`.
- Controllo all'avvio e a intervalli; rinnovo quando mancano meno di circa 12 ore.
- Nelle Impostazioni: «Sessione Developer Mode: scade tra N h» e interruttore «Rinnova automaticamente».
- Token conservato cifrato con DPAPI.

Da verificare sulla TV reale: durata effettiva dell'estensione, eventuale cookie o parametri
aggiuntivi, comportamento a sessione già scaduta (potrebbe servire riattivarla dalla TV).
Fonte degli indirizzi: stringhe di `webos-dev-manager.exe`.

## Invio file reale e installazione app

Con SSH: copiare file in `/media/developer` e installare pacchetti `.ipk`, invece di aprire solo
un URL nel browser della TV. Lettura dei log di sistema.

## Interfaccia

- Icona della tray che cambia stato (TV raggiungibile o no).
- Conferma prima di «Spegni».
- Campo opzionale «Importa chiave» per riusare una chiave SSAP già esistente senza rifare il pairing.

## Da verificare sulla TV reale (funzioni già presenti)

- Invio file: apertura sulla TV con `system.launcher/open`.
- Tastiera PC → TV su un campo di testo reale (accenti, tasti morti, Ctrl+V).
- Scorrimento con la rotella (`type: scroll`, non documentato).
- Installazione dall'installer e avvio automatico con Windows.
