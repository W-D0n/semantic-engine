# Performance et benchmark

La validation live doit rester réactive sans rendre le résultat opaque. Le
module `semantic-engine-service` ajoute donc deux mécanismes distincts :

- la **déduplication** rejoue la même validation pour une identité
  `(round_id, message_id)` et refuse un contenu contradictoire ;
- le **cache** réutilise un calcul pour une autre identité seulement si le round,
  le texte exact et la version de contexte sont identiques.

Le cache ne constitue pas une mémoire d’apprentissage. Il est local au processus,
borné à 1 024 entrées, ordonné LRU et doté d’un TTL de 10 minutes. Le nettoyage
des entrées expirées intervient à la prochaine validation. La purge de l’audit
vide également le cache et les validations volatiles.

## Mesure de référence

Commande exécutée le 1er août 2026 avec Rust 1.97.1 en profil `--release`, le
paquet starter de 84 titres, trois soumissions et 1 000 itérations, soit 3 000
échantillons par chemin :

```powershell
cargo run --release -q -p semantic-engine-cli -- benchmark `
  --package packages/starter-titles/datapackage.json `
  --submissions examples/submissions.jsonl `
  --iterations 1000
```

| Chemin | p50 | p95 | p99 | Maximum |
|---|---:|---:|---:|---:|
| cœur lexical seul | 157,1 µs | 190,3 µs | 208,2 µs | 268,5 µs |
| service, cache désactivé | 583,1 µs | 1 057,6 µs | 1 115,7 µs | 1 404,6 µs |
| service, cache chaud | 399,8 µs | 871,5 µs | 928,1 µs | 1 294,2 µs |

Le chemin chaud réduit ici le p50 du service d’environ 31 %, le p95 de 18 % et
le p99 de 17 %, tout en incluant la projection et l’écriture d’audit SQLite. Ce
résultat est une référence locale, pas une promesse universelle : une release
publique devra exécuter le même protocole sur plusieurs classes de machines et
sur un flux plus représentatif avant de fixer un SLO.

## Garde-fous

- Les clés sont des SHA-256 en mémoire ; aucun texte brut supplémentaire n’est
  persisté par le cache.
- Une policy non finie ou une entrée invalide continue de produire le verdict
  contractuel du cœur au lieu de casser la génération de clé.
- La capacité zéro désactive explicitement le cache pour comparer les chemins.
- Compter les hits ne suffit pas : suivre p50/p95/p99, taux de conflit, taille du
  contexte et pression d’écriture de l’audit.
