# Kit de conformité des clients

Ce dossier vérifie qu’un client indépendant peut piloter Semantic Engine sans
importer ses crates Rust ni dépendre de Tauri. Le client de référence Node.js
n’utilise aucune dépendance npm : il ne connaît que les enveloppes JSONL
publiques de `contracts/`.

## Exécution

Construire la CLI puis lancer le client :

```powershell
cargo build -p semantic-engine-cli
node conformance/clients/node-client.mjs target/debug/semantic-engine-cli.exe
```

Le scénario vérifie :

- corrélation stricte d’une réponse par requête ;
- création, soumission, événements et fin de session ;
- reprise et idempotence après redémarrage du sidecar ;
- conflit sur une identité rejouée avec un autre contenu ;
- minimisation du journal d’événements ;
- refus des écritures après fermeture ;
- limites de taille côté client et délais bornés.

La base temporaire est supprimée à la fin. Un futur adaptateur HTTP/WebSocket
devra passer les mêmes assertions sémantiques, seules les fonctions de transport
étant remplacées.
