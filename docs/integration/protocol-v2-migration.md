# Migration du protocole v1 vers v2

Le protocole v2 introduit la mémoire de reconnaissance consentie. Il ajoute les
commandes `remember_resolution`, `list_memory` et `revoke_memory`, ainsi que la
preuve `memory_expression`. Cette nouvelle valeur rend les consommateurs v1 à
énumération exhaustive incompatibles ; la version est donc explicitement augmentée.

## Adaptation d'un client

1. envoyer `protocol_version: 2` dans les enveloppes JSONL et HTTP ;
2. envoyer l'en-tête `X-Semantic-Engine-Protocol: 2` en loopback ;
3. proposer `semantic-engine.v2` comme sous-protocole WebSocket ;
4. accepter `contract_version: 2` pour sessions et événements, et
   `schema_version: 2` pour l'audit ;
5. traiter `memory_expression` comme une acceptation exacte issue d'une correction
   opérateur, distincte d'un alias configuré ;
6. pour apprendre, transmettre seulement `session_id` et `message_id`. Le texte et
   la cible viennent du backend ;
7. pour révoquer, renvoyer le SHA-256 du contexte et l'identifiant opaque reçu.

Les chemins HTTP restent sous `/v1/` : ils versionnent la surface de transport,
tandis que l'en-tête négocie le contrat métier. Les anciennes sessions SQLite sont
relues ; leurs nouveaux événements et snapshots sont ensuite émis en v2. Aucune
réautorisation Twitch ou YouTube n'est requise.
