# ADR 0009 — Cycle de session et événements publics

- **Statut** : accepté
- **Date** : 2026-08-01

## Contexte

Une validation isolée ne suffit pas à un jeu live. Le client doit savoir quelle
manche et quelle version de contexte gouvernent les messages, quand la manche est
fermée et quels signaux ont déjà été remis au workflow. Ce besoin doit rester
indépendant de Twitch, Tauri, MyVault et d’un futur serveur HTTP.

## Décision

Le module d’application possède un cycle versionné : démarrer, consulter,
soumettre, arbitrer, lire les événements et terminer. Une session lie une manche
immuable à l’empreinte facultative d’un paquet de contexte. Son identifiant est
idempotent pour une définition identique et conflictuel pour une redéfinition.

Chaque changement utile crée un événement à séquence monotone. Les événements de
validation ne contiennent ni texte du chat ni expression correspondante. La
rétention en mémoire est bornée ; les pages signalent explicitement une lacune
avec `truncated` et la première séquence encore disponible.

`semantic-engine-protocol` adapte ces opérations vers des enveloppes versionnées.
JSONL est le premier transport ; Tauri appelle le même service en mémoire et le
futur HTTP/WebSocket devra conserver la même sémantique.

## Conséquences

- un workflow de score peut consommer des validations sans entrer dans le cœur ;
- changer de cible ou de contexte exige une nouvelle session ;
- un client doit traiter les conflits, fins de session et lacunes d’événements ;
- la reprise après redémarrage n’est pas encore garantie et reste un travail M2 ;
- toute persistance future conserve la minimisation des données et les limites.
