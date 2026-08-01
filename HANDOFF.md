# Handoff — Semantic Engine

## Mise à jour d'architecture du 31 juillet 2026

Semantic Engine est un produit autonome. MyVault, OBS, les bots et les webapps
sont des consommateurs optionnels équivalents ; aucun n'appartient au chemin
critique. Answer Atlas et le nouveau dépôt `C:\DEV\media-catalog` sont des
ressources facultatives, jamais des dépendances d'exécution.

Les contrats publics vivent dans `contracts/`. JSONL est le transport disponible.
HTTP/WebSocket est un adaptateur embarqué, opt-in, lié à `127.0.0.1` et
désactivé par défaut. Tauri continue d'appeler le moteur en mémoire.

Lire en priorité `docs/architecture/overview.md`,
`docs/adr/0006-autonomous-public-interface.md` et `docs/roadmap.md`.

## Objectif de la prochaine session

Conduire un pilote YouTube réel consenti, mesurer p50/p95/p99 réseau et préparer
le dossier de conformité avant d’ouvrir les verdicts/points dans la distribution.
La gestion de sources est disponible par Tauri et par l’API loopback/headless
sans exposer les jetons ; plusieurs sources partagent un ordre de session global
et durable.

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
- module `semantic-engine-service` partagé par Tauri, avec identité idempotente,
  conflits explicites et cache 1 024 entrées/10 minutes partitionné par contexte ;
- cycle de session v1 lié à une manche et à une empreinte de contexte, contrôlable
  depuis Tauri et journalisé en événements minimisés à séquence monotone ;
- store de sessions SQLite normalisé : reprise de l’état actif dans la portable,
  rejeu et arbitrage idempotents après redémarrage, sans texte du chat ;
- crate `semantic-engine-protocol` et commande `serve` : JSONL corrélé, versionné,
  borné à 1 Mio par ligne et capable de continuer après une requête invalide ;
- kit `conformance/` avec un client Node sans dépendance au code Rust, validant
  cycle, redémarrage, erreurs et confidentialité sur le binaire réel ;
- crate `semantic-engine-loopback` : HTTP/WebSocket opt-in, bind loopback imposé,
  jeton éphémère, origines exactes, quotas, refus sous backpressure et API sources ;
- crate `semantic-engine-source-runtime` : orchestration Twitch partagée entre
  Tauri et CLI headless, séquence globale durable multi-source ;
- second client Node indépendant couvrant santé, auth, HTTP, WebSocket,
  idempotence et absence de chat brut dans SQLite ;
- benchmark CLI reproductible p50/p95/p99 ; sur 84 titres, le cache chaud réduit
  le p50 du service de 583,1 µs à 399,8 µs sur la machine de référence ;
- portable Tauri hors ligne avec WebView2 fixe, checksums et lanceur racine ;
- variante légère `SemanticEngine.exe` toujours disponible ;
- source générique SQLite révisionnée, sans secrets dans les paramètres ;
- coffre natif Windows/macOS/Linux avec buffers bornés, redaction et zeroization ;
- OAuth Twitch Device Code public, scope `user:read:chat`, validation horaire et rotation ;
- EventSub WebSocket avec reconnexion, déduplication bornée et backpressure ;
- UI d’ajout, autorisation, test, écoute, pause et suppression Twitch ;
- validations live visibles et arbitrables sans conserver le texte du chat ;
- API publique de gestion des sources et agrégation multi-source ordonnée ;
- CI publique, SBOM CycloneDX, threat model et politiques de contribution/sécurité ;
- benchmark loopback live (500 messages) : p50 15,95 ms, p95 23,21 ms,
  p99 32,30 ms sur la machine de référence ;
- YouTube OAuth Desktop PKCE, découverte/sélection des lives de la chaîne,
  `streamList` gRPC, checkpoint durable, reprise et fautes quota/auth typées ;
- corpus qualité v2 : 84 titres, 328 annotations multi-catégories, quality gate
  précision/rappel obligatoire, résultat de référence 1,0/1,0 sans faux positif ;
- le pilote YouTube réel, l’audit de conformité, la release signée et le
  scoreboard consommateur restent à faire.

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
15. `crates/semantic-engine-service/src/lib.rs`
16. `docs/product/performance.md`
17. `crates/semantic-engine-protocol/src/lib.rs`
18. `docs/integration/jsonl-sidecar.md`
19. `docs/integration/twitch.md`
20. `crates/semantic-engine-source/src/lib.rs`
21. `crates/semantic-engine-source-runtime/src/lib.rs`
22. `crates/semantic-engine-twitch/src/lib.rs`
23. `apps/desktop/src/lib/SourcePanel.svelte`
24. `docs/integration/youtube.md`
25. `crates/semantic-engine-youtube/src/lib.rs`

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
cargo run -q -p semantic-engine-cli -- evaluate `
  --titles tests/corpus/titles.json --cases tests/corpus/cases.json `
  --minimum-precision 0.95 --minimum-recall 0.90
cargo run -q -p semantic-engine-cli -- context validate `
  --package packages/starter-titles/datapackage.json
node conformance/clients/node-client.mjs target/debug/semantic-engine-cli.exe
node conformance/clients/node-loopback-client.mjs target/debug/semantic-engine-cli.exe
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

Réaliser un pilote YouTube réel consenti, mesurer sa latence et préparer l’audit
de conformité ; faire ensuite arbitrer la licence du code et publier une release
signée. Ne pas faire transiter les jetons dans JSONL/HTTP et ne pas réintroduire
de dépendance vers MyVault, de chat brut persistant ni de scoreboard dans le moteur.
