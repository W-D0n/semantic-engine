# ADR 0006 — Produit autonome et interface publique locale

- Statut : accepté
- Date : 2026-07-31

## Contexte

Semantic Engine doit être utilisable comme application portable complète et
pouvoir alimenter des workflows externes. Présenter une webapp particulière
comme orchestrateur privilégié introduirait un lien conceptuel fort et réduirait
l'autonomie du produit.

L'expression « API publique » peut aussi être trompeuse : un contrat public et
stable n'implique pas qu'un port soit ouvert sur Internet ni même que le produit
ait besoin du réseau pour fonctionner.

## Décision

Semantic Engine possède son interface publique et demeure l'unique propriétaire
de son orchestration de reconnaissance.

- Les contrats indépendants du transport vivent dans `contracts/`.
- Le cœur Rust ne connaît aucun consommateur externe.
- Tauri appelle le module d'application en mémoire par IPC privé.
- Le sidecar JSONL est le premier transport public local.
- HTTP/WebSocket sera un adaptateur optionnel embarqué, désactivé par défaut et
  lié à `127.0.0.1`.
- Un hôte headless pourra réutiliser cet adaptateur sans modifier le cœur.
- Les intégrations spécifiques appartiennent au consommateur ou à un dépôt
  d'adaptateur séparé.

MyVault n'a aucun statut privilégié. Il constitue un exemple de consommateur au
même titre qu'une webapp, un bot ou OBS.

## Invariants

1. L'application portable reste complète sans API réseau active.
2. Aucun type, identifiant ou règle métier d'un consommateur n'entre dans le cœur.
3. Les transports partagent les mêmes contrats versionnés et tests de conformité.
4. Une intégration dépend de Semantic Engine ; Semantic Engine ne dépend pas de
   l'intégration.
5. Answer Atlas et Media Catalog sont des ressources facultatives, jamais des
   dépendances d'exécution.
6. Une exposition autre que loopback est un mode distinct avec sécurité explicite.

## Conséquences

- Le produit peut être distribué, testé et utilisé seul.
- Une intégration locale peut commencer avec JSONL, puis adopter HTTP/WebSocket
  sans changer les concepts métier.
- Le futur adaptateur réseau nécessite jeton local, contrôle d'origine, limites,
  audit et désactivation simple.
- Le scoreboard, la désignation du gagnant et les règles d'émission restent chez
  les consommateurs.

## Alternatives rejetées

- Faire de MyVault le plan de contrôle : couplage à un produit et à son cycle de vie.
- Faire dépendre Tauri d'un serveur loopback : complexité et surface d'attaque
  inutiles pour le mode autonome.
- Mettre l'interface publique dans Media Catalog : responsabilités et cycles de
  données différents.
- Publier immédiatement un service Internet : sécurité et exploitation avant
  validation du besoin.
