# Contribuer

Merci de garder Semantic Engine local-first, modulaire et indépendant d'un hub,
d'un catalogue ou d'une plateforme particulière.

## Avant une modification

1. Ouvrir une issue concise pour les changements de contrat ou d'architecture.
2. Ne jamais ajouter de jeton, export de chat, cache fournisseur ou donnée dont
   la redistribution n'est pas autorisée.
3. Préserver la compatibilité des schémas publics ; une rupture exige une
   nouvelle version de contrat et une note de migration.
4. Ajouter un test au niveau le plus bas pertinent, puis un test de transport si
   l'interface publique change.

## Vérification locale

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd apps/desktop
npm ci
npm run check
npm run build
```

Pour un changement HTTP/JSONL, exécuter aussi les clients Node dans
`conformance/clients/`. Une pull request décrit le problème, la décision, les
risques, les tests et l'effet sur sécurité/confidentialité.

## Données et contexte

Les paquets partagés utilisent les conventions de
`docs/integration/context-packages.md`. Les nouvelles données indiquent leur
source, licence, transformations et limites. Answer Atlas et Media Catalog sont
des producteurs facultatifs, jamais des dépendances d'exécution.

L'acceptation d'une contribution n'accorde aucun droit sur des marques, données
tierces ou contenus de plateforme. La licence du code du dépôt doit être décidée
avant l'acceptation de contributions externes.
