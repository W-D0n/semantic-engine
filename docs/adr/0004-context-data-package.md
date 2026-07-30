# ADR-0004 — Diffuser les contextes comme Data Packages

- Statut : accepté
- Date : 2026-07-30

## Contexte

Les titres, alias et abréviations doivent être importables, auditables et
diffusables par des tiers. Un JSON interne seul ne définit ni identité stable,
ni version, ni licence, ni provenance, ni intégrité.

## Décision

Un contexte partageable est un Data Package v2 avec un profil Semantic Engine
en JSON Schema 2020-12. Le manifeste `datapackage.json` déclare identité, SemVer,
sources, licence, ressources, taille et SHA-256. La ressource `targets` contient
les données de reconnaissance et aucun code exécutable.

L’importeur Rust valide les limites, le profil, les chemins relatifs confinés,
la taille déclarée, le hash, la version et la structure métier avant exposition.
L’activation persistante restera une opération séparée et transactionnelle.

## Conséquences

- les paquets sont lisibles par les outils Data Package génériques ;
- un dossier, un ZIP ou une URL statique suffit pour publier ;
- données et moteur évoluent indépendamment ;
- la licence des données est explicite et distincte du code ;
- les versions sont immuables et permettent cache et rollback ;
- hash et schéma n’établissent pas la confiance dans l’éditeur.

La signature authentifiée et le catalogue public sont différés. Une future
signature canonisera le JSON selon RFC 8785 avant signature afin d’éviter les
variations d’ordre et d’espacement.

## Alternatives

JSON ad hoc, paquet de code, SQLite brut et embeddings obligatoires ont été
écartés : ils offrent moins d’interopérabilité, mélangent données et exécution ou
augmentent inutilement le coût du MVP.
