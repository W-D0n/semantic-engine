# ADR-0007 — Audit persistant minimisé

- **Statut** : accepté
- **Date** : 2026-08-01

## Contexte

L’arbitrage doit survivre au redémarrage et devenir consommable par des clients
indépendants. Conserver la soumission complète simplifierait le diagnostic, mais
transformerait le journal en historique de chat et augmenterait fortement le
risque légal, la surface de fuite et le coût de rétention.

## Décision

Un module Rust autonome persiste dans SQLite une projection versionnée de la
validation et sa résolution éventuelle. L’identité `(round_id, message_id)` est
unique et immuable. L’ordre de réception local et `source_sequence` sont tous
deux conservés. Les reprises identiques sont idempotentes ; les reprises
contradictoires échouent.

La projection exclut le texte de `Submission` et `Evidence.matched_expression`.
La rétention par défaut est limitée à 10 000 validations et 30 jours. Une purge
explicite est exposée à l’opérateur. Le stockage ne dépend pas de Tauri.

## Conséquences

- Un workflow peut attribuer des points avec l’identité, l’ordre et le verdict.
- L’explication historique conserve le type de preuve et le score, mais pas la
  formulation exacte ; le diagnostic approfondi doit être reproduit avec un
  consentement et un mécanisme distincts.
- Les identifiants de participant restent des données à protéger.
- Une modification d’arbitrage future devra produire une nouvelle version ou un
  événement correctif ; elle ne pourra pas écraser silencieusement la résolution.
