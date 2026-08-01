# API locale HTTP et WebSocket

La passerelle loopback expose le même protocole public v1 que le sidecar JSONL.
Elle est indépendante de Tauri, désactivée par défaut et ne peut écouter que sur
une adresse loopback. Son contrat source se trouve dans
`contracts/loopback-openapi.yaml` et les corps réutilisent les schémas JSON de
`contracts/`.

## Démarrage explicite

```powershell
cargo run -p semantic-engine-cli -- loopback --enable `
  --audit .\semantic-engine.sqlite3 `
  --port 17831 `
  --origin http://localhost:5173
```

Le premier objet JSON écrit sur stdout contient l'adresse effective, la version
et un jeton aléatoire éphémère de 256 bits. Le port `0` demande au système un port
libre. Ne pas placer le jeton dans une URL, un fichier versionné ou un log.

## Commandes HTTP

`POST /v1/commands` attend :

- `Authorization: Bearer <jeton>` ;
- `X-Semantic-Engine-Protocol: 1` ;
- une origine figurant dans l'allowlist si le client envoie `Origin` ;
- un corps conforme à `contracts/protocol-request.schema.json` et inférieur ou
  égal à 1 Mio.

```js
const response = await fetch(`${address}/v1/commands`, {
  method: "POST",
  headers: {
    Authorization: `Bearer ${token}`,
    "Content-Type": "application/json",
    "X-Semantic-Engine-Protocol": "1",
  },
  body: JSON.stringify({
    protocol_version: 1,
    request_id: crypto.randomUUID(),
    command: "stats",
  }),
});
```

Les erreurs métier restent des réponses corrélées HTTP 200 avec `status: error`.
Les erreurs de transport utilisent 400, 401, 403, 413, 415, 426, 429, 500 ou
503 et un objet `error` stable. `GET /v1/health` ne retourne ni donnée de session
ni secret.

## Événements WebSocket

Le client se connecte à
`/v1/events/ws?session_id=...&after_sequence=0&limit=100` et propose deux
sous-protocoles :

```js
const socket = new WebSocket(url, [
  "semantic-engine.v1",
  `semantic-engine.token.${token}`,
]);
```

Le serveur sélectionne uniquement `semantic-engine.v1`. Le second protocole sert
à authentifier les navigateurs, qui ne peuvent pas définir librement un header
`Authorization` pendant l'upgrade. Chaque message reçu est une enveloppe de
réponse v1 contenant une page d'événements. `after_sequence` permet la reprise
sans doublon après une reconnexion.

```mermaid
sequenceDiagram
    participant C as Client local
    participant L as Passerelle loopback
    participant S as Service partagé
    C->>L: POST /v1/commands + Bearer + version 1
    L->>S: RequestEnvelope v1
    S-->>L: ResponseEnvelope v1
    L-->>C: JSON corrélé
    C->>L: Upgrade WS + origine + sous-protocoles
    loop Nouveaux événements
        L->>S: events(after_sequence)
        S-->>L: page minimisée
        L-->>C: ResponseEnvelope v1
    end
```

## Limites et modèle de sécurité

- bind non-loopback refusé par la bibliothèque ;
- jeton généré par le CSPRNG du système, comparé via son hash et jamais écrit en
  base ;
- aucune origine navigateur autorisée implicitement ; chaque origine est exacte
  et ajoutée explicitement, avec un CORS non permissif ;
- 100 requêtes/s, 32 requêtes en vol et 8 WebSockets par défaut ;
- refus immédiat sous charge, sans file non bornée ;
- pages d'événements limitées à 1 000 et polling interne borné ;
- aucun texte brut du chat ni expression correspondante dans les événements.

Ce transport n'est ni une API LAN ni une API Internet. Un futur hôte headless
réutilisera le module `semantic-engine-loopback`, mais devra avoir un mode réseau
distinct avec TLS, identité durable, rôles, rotation et observabilité adaptée.

## Conformité

```powershell
cargo build -p semantic-engine-cli
node conformance/clients/node-loopback-client.mjs target/debug/semantic-engine-cli.exe
```

Le client Node lance le binaire réel, vérifie auth, santé, HTTP, WebSocket,
idempotence et minimisation, puis confirme que SQLite ne contient pas le message
privé utilisé par le scénario.
