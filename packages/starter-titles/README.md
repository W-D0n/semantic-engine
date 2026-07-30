# Starter Titles

Paquet de contexte de démonstration pour Semantic Engine : 20 films avec alias
VO/VF et 64 jeux vidéo.

Le contenu est généré par `scripts/build-title-corpus.mjs`. Ne pas modifier
`data/titles.json`, `profile/*.json` ou les champs `bytes`/`hash` du manifeste à
la main ; modifier le générateur puis le relancer.

## Distribution

Distribuer le dossier complet ou une archive ZIP qui conserve `datapackage.json`
à la racine. Un importeur doit vérifier le profil, le chemin relatif, la taille et
le SHA-256 avant de lire les titres.

## Licence

Ce paquet de démonstration est placé sous CC0-1.0. Les titres et alias ont été
curatés pour les tests du projet ; ils ne proviennent pas des datasets IMDb,
TMDB ou IGDB.
