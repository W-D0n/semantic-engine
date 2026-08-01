# Handoff — Semantic Engine

## Mise à jour d'architecture du 31 juillet 2026

Semantic Engine est un produit autonome. MyVault, OBS, les bots et les webapps
sont des consommateurs optionnels équivalents ; aucun n'appartient au chemin
critique. Answer Atlas et le nouveau dépôt `C:\DEV\media-catalog` sont des
ressources facultatives, jamais des dépendances d'exécution.

Les contrats publics vivent dans `contracts/`. JSONL est le transport disponible.
HTTP/WebSocket sera un adaptateur embarqué, opt-in, lié à `127.0.0.1` et
désactivé par défaut. Tauri continuera d'appeler le moteur en mémoire.

Lire en priorité `docs/architecture/overview.md`,
`docs/adr/0006-autonomous-public-interface.md` et `docs/roadmap.md`.

## Objectif de la prochaine session

Implémenter la déduplication et le cache LRU/TTL puis mesurer p50/p95/p99.
L’audit persistant minimisé et l’export immuable des brouillons sont livrés.

## État au 1er août 2026

- moteur Rust déterministe avec normalisation, alias, fautes, marge et abstention ;
- contrats `Submission`, `Validation` et résolution opérateur ;
- corpus CC0 de 84 titres et suite contractuelle Rust reproductible ;
- dépôt public [Answer Atlas](https://github.com/W-D0n/answer-atlas) propriétaire
  du catalogue diffusable, avec IDs qualifiés, build, validation et CI ;
- paquet Data Package v2 vérifié par SemVer, SPDX, limites, chemins et SHA-256 ;
- inspection, activation SQLite immuable, état actif au redémarrage et rollback ;
- recherche bornée dans le contexte actif et brouillons locaux persistants ;
- export d’une nouvelle version immuable, atomique et réimportable depuis l’atelier,
  avec profils hors ligne, README, notice de licence et checksums ;
- atelier Svelte pour modifier canonique/alias, restaurer le publié et choisir la cible du round ;
- arbitrage manuel accepter/rejeter avec note, sans effacer la décision moteur ;
- audit SQLite idempotent des validations/résolutions, ordonné par source,
  borné à 10 000 entrées et 30 jours, purgeable sans conserver le chat brut ;
- portable Tauri hors ligne avec WebView2 fixe, checksums et lanceur racine ;
- variante légère `SemanticEngine.exe` toujours disponible ;
- Twitch, YouTube, auth, cache et scoreboard non implémentés.

## Lire d’abord

1. `README.md`
2. `CONTEXT.md`
3. `docs/product/portable-desktop.md`
4. `docs/integration/context-packages.md`
5. `docs/adr/0005-local-context-drafts.md`
6. `docs/roadmap.md`
7. `C:\DEV\answer-atlas\CONTEXT.md`
8. `C:\DEV\answer-atlas\docs\conventions.md`
9. `crates/semantic-engine-context-store/src/lib.rs`
10. `crates/semantic-engine-core/src/lib.rs`
11. `apps/desktop/src/lib/ContextWorkshop.svelte`
12. `apps/desktop/src/lib/ArbitrationPanel.svelte`
13. `docs/product/audit.md`
14. `crates/semantic-engine-audit-store/src/lib.rs`

## Décisions à préserver

- les données s’appellent **paquet de contexte**, pas dictionnaire global ;
- une version publiée est immuable ; un réglage local est un calque séparé ;
- aucun paquet ne contient ni n’exécute du code ;
- import, aperçu, activation, brouillon et export sont des opérations distinctes ;
- une acceptation opérateur référence une cible du round et conserve la décision moteur ;
- l’audit conserve les identifiants, décisions, scores et catégories de preuve,
  jamais le texte brut ni l’expression correspondante ;
- les versions et brouillons sont stockés dans l’AppData, pas avec l’exécutable ;
- le moteur ne désigne pas le gagnant et ne compte pas les points ;
- vecteurs, microservices et réécriture Rust restent ouverts après benchmark ;
- le package hors ligne doit prouver au lancement le chemin du runtime embarqué ;
- avant chaque clôture, synchroniser les erreurs réutilisables avec `C:\DEV\error-tracking\README.md`.

## Artefacts opérateur

- lancement autonome : `SemanticEngine Portable.cmd` ;
- package généré : `portable/SemanticEngine` ;
- variante légère : `SemanticEngine.exe` ;
- source du runtime verrouillée : `scripts/webview2-runtime.json` ;
- générateur : `scripts/build-portable.ps1`.

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
cd ../..
python -m mkdocs build --strict
```

Pour la portable, vérifier `SHA256SUMS.txt`, lancer le raccourci racine et
confirmer que le processus `msedgewebview2.exe` provient de
`portable/SemanticEngine/WebView2`.

## Première action recommandée

Écrire d’abord les tests de déduplication par identité de soumission et de cache
borné par version de contexte. Mesurer ensuite p50/p95/p99 sur le corpus avant
d’exposer ces mécanismes dans le futur module d’application. Le journal d’audit
est un module Rust autonome : ne pas réintroduire de dépendance vers Tauri ni y
copier le texte brut du chat.
