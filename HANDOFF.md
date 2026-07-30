# Handoff — Semantic Engine

## Objectif de la prochaine session

Ajouter l’activation atomique du paquet déjà inspecté, persister la version
active dans SQLite et conserver la version précédente pour un rollback explicite.

## État au 30 juillet 2026

- moteur Rust déterministe avec normalisation, alias, fautes et abstention ;
- ambiguïtés exactes ou fuzzy protégées par marge au second candidat ;
- crate `semantic-engine-package` et commande CLI de validation ;
- profil Data Package v2 / JSON Schema 2020-12 et corpus CC0 de 84 titres ;
- contrôle SemVer, SPDX, tailles, chemins confinés, SHA-256 et structure ;
- console Tauri/Svelte et sidecar JSONL opérationnels ;
- sélection native de `datapackage.json`, inspection Rust et aperçu inactif ;
- exécutable léger compilé dans `target/release` puis copié en
  `SemanticEngine.exe` à la racine pour l’opérateur ;
- Twitch, YouTube, auth, cache, SQLite et scoreboard non implémentés.

## Lire d’abord

1. `README.md`
2. `CONTEXT.md`
3. `docs/integration/context-packages.md`
4. `docs/adr/0004-context-data-package.md`
5. `docs/roadmap.md`
6. `crates/semantic-engine-package/src/lib.rs`
7. `crates/semantic-engine-core/src/lib.rs`

## Décisions à préserver

- les données s’appellent **paquet de contexte**, pas dictionnaire global ;
- aucune ressource de paquet ne contient ni n’exécute du code ;
- une version publiée est immuable ; hash et signature ont des rôles distincts ;
- import, aperçu et activation sont trois opérations séparées ;
- le moteur ne désigne pas le gagnant et ne compte pas les points ;
- vecteurs et microservices restent possibles, mais après benchmark/justification ;
- la variante portable actuelle dépend de WebView2 système ; fixed runtime est à faire.
- avant chaque clôture, synchroniser les erreurs réutilisables avec `C:\DEV\error-tracking\README.md`.

## Vérifications de référence

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -q -p semantic-engine-cli -- context validate `
  --package packages/starter-titles/datapackage.json
cd apps/desktop
npm run check
npm run build
npm run tauri -- build --no-bundle
```
Valider séparément le registre d’erreurs avec les commandes qu’il contient.


Depuis la racine, `python -m mkdocs build --strict` vérifie le guide. Le générateur
`node scripts/build-title-corpus.mjs` doit rester déterministe : le hash actuel de
`data/titles.json` est
`062cc8a6223685ac8fb0d6112b8393a5d849dd8d4dcba648dac719679a82b8c1`.

## Première action recommandée

Écrire d’abord le test du flux `inspect → activate → rollback` contre une copie
temporaire de SQLite. Réutiliser `inspect_context_package` comme frontière validée,
puis exposer une commande d’activation séparée et idempotente. Ne pas élargir les
capabilities du frontend : le backend reste seul responsable des écritures.
