# Semantic Engine

Moteur local-first, explicable et modulaire qui transforme des messages de chat
imparfaits en interprétations configurées : fautes, abréviations, initiales et
formulations proches comprises.

> État actuel : moteur Rust, CLI JSONL et client Tauri portable hors ligne avec
> inspection/rollback des paquets, tuning local des titres et arbitrage opérateur.
> Le corpus contractuel couvre 84 titres et 28 cas de validation.

## Pourquoi ce projet

Les intégrations Twitch, YouTube, webhook ou terminal produisent toutes des
messages différents. Semantic Engine isole leur transport du vrai problème :
reconnaître ce que l’utilisateur veut dire, indiquer pourquoi, et s’abstenir
quand la réponse n’est pas assez sûre.

## Guide

- [Vue d’ensemble](docs/index.md)
- [Vision produit](docs/product/vision.md)
- [Architecture modulaire](docs/architecture/overview.md)
- [Pipeline de reconnaissance](docs/architecture/recognition-pipeline.md)
- [Importer et diffuser un paquet de contexte](docs/integration/context-packages.md)
- [Conventions techniques retenues](docs/research/context-package-conventions.md)
- [Sécurité, safety et cadre légal](docs/product/security-and-legal.md)
- [Ouvertures produit et marché](docs/product/market.md)
- [Solutions et corpus existants](docs/research/existing-solutions-and-data.md)
- [Intégration locale JSONL et contrat de résolution](docs/integration/jsonl-sidecar.md)
- [Application portable](docs/product/portable-desktop.md)
- [Roadmap](docs/roadmap.md)
- [Continuité entre sessions et outils IA](docs/contributing/session-continuity.md)
- [Handoff courant](HANDOFF.md)
- [Glossaire métier](CONTEXT.md)
- [Décisions d’architecture](docs/adr/)

## Principes directeurs

1. **Produit utilisateur avant démonstration technique** : une installation
   locale utile prévaut sur une architecture spectaculaire.
2. **Précision avant couverture** : une abstention explicite vaut mieux qu’une
   interprétation fausse.
3. **Local-first et explicable** : les règles, données et décisions restent
   inspectables ; aucun service IA distant n’est requis par défaut.
4. **Moteur séparé du contexte** : le cœur ne connaît ni Twitch, ni YouTube, ni
   le métier qui configure les interprétations.
5. **Entrées non fiables** : messages, webhooks, fichiers, modèles et feedbacks
   sont validés avant usage.
6. **Ouvert publiquement dès le départ** : documentation, exemples, décisions et
   critères d’acceptation font partie du produit.
7. **Autonome par construction** : MyVault, Media Catalog, Answer Atlas et les
   autres clients restent des ressources ou consommateurs facultatifs.

## Essai rapide

```powershell
# Tous les tests du moteur et du corpus
cargo test -p semantic-engine-core

# Sidecar local : une soumission JSON par ligne, une validation par ligne
Get-Content examples/submissions.jsonl |
  cargo run -q -p semantic-engine-cli -- validate --round examples/rounds/elden-ring.json

# Vérifier un paquet de titres avant de l’activer
cargo run -q -p semantic-engine-cli -- context validate `
  --package packages/starter-titles/datapackage.json

# Client Tauri en développement
cd apps/desktop
npm install
npm run tauri dev
```

Pour l’usage opérateur, double-cliquer sur **`SemanticEngine Portable.cmd`** à la
racine. Le dossier `portable/SemanticEngine` contient l’exécutable, WebView2 fixe
et les checksums : aucune installation ni aucun téléchargement au premier lancement.
La variante légère `SemanticEngine.exe` reste disponible et utilise WebView2 système.

Dans l’app : activer un paquet, ouvrir **Voir et régler le dictionnaire**, rechercher
un titre, modifier canonique/alias et enregistrer le brouillon local. Après une
validation, **Arbitrage manuel** permet d’accepter ou rejeter sans effacer la
décision du moteur. Voir le [guide portable](docs/product/portable-desktop.md).

Le catalogue public de titres vit dans
[Answer Atlas](https://github.com/W-D0n/answer-atlas). Son paquet
`packages/core-titles/datapackage.json` est directement importable ; le corpus
embarqué ici reste un fixture contractuel du moteur.

## Documentation locale

La documentation est lisible directement sur GitHub. Pour une navigation locale
avec recherche :

```bash
python -m pip install -r docs/requirements.txt
python -m mkdocs serve
```

Puis ouvrir `http://127.0.0.1:8000`.
