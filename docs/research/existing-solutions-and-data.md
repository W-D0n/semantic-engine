# Solutions, dictionnaires et données existantes

Recherche effectuée le 30 juillet 2026 à partir de sources officielles.

## Conclusion

Il n’existe pas de dictionnaire universel qui transforme automatiquement toute
réponse imparfaite en bon titre. Il existe en revanche trois familles de briques :
catalogues de titres/alias, algorithmes de similarité et embeddings locaux.

Le produit doit assembler ces briques et calibrer ses seuils sur son propre corpus.

## Données de titres

- [Wikidata](https://www.wikidata.org/wiki/Wikidata:Data_access) fournit labels
  et alias multilingues sous CC0. C’est la meilleure base ouverte pour une
  commercialisation, avec cache local et provenance.
- [IMDb datasets](https://www.imdb.com/interfaces/) expose `title.basics` et
  `title.akas`, mais seulement pour usage personnel et non commercial.
- [TMDB](https://developer.themoviedb.org/reference/movie-alternative-titles)
  fournit les titres alternatifs ; le gratuit est non commercial avec attribution.
- [IGDB](https://api-docs.igdb.com/) recherche jeux et variantes ; son API
  gratuite est annoncée pour usage non commercial sous accord Twitch.
- Le catalogue MyVault contient 1 207 jeux et sert de source de test locale ; sa
  provenance IGDB interdit de présumer qu’il est redistribuable ouvertement.

## Algorithmes et projets

- [strsim](https://docs.rs/strsim/) : Levenshtein, Damerau-Levenshtein,
  Jaro-Winkler et Sørensen-Dice en Rust.
- [unicode-normalization](https://docs.rs/unicode-normalization/) : NFC, NFD,
  NFKC et NFKD.
- [RapidFuzz](https://rapidfuzz.github.io/RapidFuzz/) : référence MIT performante
  côté Python/C++, utile comme oracle de comparaison.

Ces briques ne réunissent pas à elles seules arbitrage temps réel, alias VO/VF,
explication, idempotence et validation de round : c’est l’espace du produit.

## Tauri et portabilité

[Tauri 2](https://v2.tauri.app/start/) assemble frontend web et cœur Rust. Sur
Windows moderne, WebView2 est généralement fourni par le système. Pour un bundle
hors ligne, Tauri documente un runtime
[Fixed Version](https://v2.tauri.app/distribute/windows-installer/). Le package
x64 vérifié dans ce projet pèse environ 690 Mo extrait ; la taille varie selon la
version. Pour un flux soutenu, les
[channels](https://v2.tauri.app/develop/calling-frontend/) sont préférables aux
événements JSON génériques.

## Décision proposée

- Développement : titres curatés et données MyVault non redistribuées.
- Catalogue ouvert futur : import Wikidata CC0.
- M1 : règles, alias, Damerau-Levenshtein et scores par tokens.
- Embeddings Rust seulement si le benchmark démontre un gain net.
