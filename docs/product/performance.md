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

## Flux live simulé de bout en bout

Le profil `benchmarks/live-loopback.mjs` lance le véritable hôte release avec
deux bases temporaires sur disque, ouvre une session de 84 titres et envoie un
mélange déterministe de réponses exactes, alias, fautes et messages hors sujet.
La mesure commence avant l'appel HTTP et se termine après lecture de la réponse
JSON ; elle inclut donc transport loopback, validation, session durable et audit.

```powershell
cargo build --release -p semantic-engine-cli
node benchmarks/live-loopback.mjs target/release/semantic-engine-cli.exe `
  --samples 500 --interval-ms 25
```

Mesure du 1er août 2026 sous Windows 10.0.26200, Ryzen 9 7950X, Node 25.2.1,
avec 500 messages, 200 participants simulés et un intervalle de 25 ms :

| Chemin live | p50 | p95 | p99 | Maximum |
|---|---:|---:|---:|---:|
| HTTP → service → SQLite → JSON | 15,95 ms | 23,21 ms | 32,30 ms | 36,76 ms |

Le script borne les échantillons à 5 000, impose au moins 11 ms entre requêtes
pour respecter le quota public et supprime son état temporaire. Il ne mesure pas
le trajet Twitch → machine : cette latence dépend du réseau et devra être ajoutée
au pilote réel. Le résultat établit seulement que le traitement local p99 reste
très inférieur à 50 ms dans ce scénario.

## Gate de qualité du moteur

Le corpus v2 contient 328 messages annotés pour 84 titres : titres exacts,
normalisations, alias, fautes curatées, 33 négatifs difficiles, 17 frontières de
revue et les cas historiques. La commande suivante compare les décisions du
moteur aux annotations et échoue si la précision des acceptations descend sous
95 % ou leur rappel sous 90 % :

```powershell
cargo run -q -p semantic-engine-cli -- evaluate `
  --titles tests/corpus/titles.json `
  --cases tests/corpus/cases.json `
  --minimum-precision 0.95 `
  --minimum-recall 0.90
```

Référence locale du 1er août 2026 : 328/328 décisions conformes, précision et
rappel des acceptations à 100 %, 0 faux positif, 18 abstentions et 36 rejets.
Cette mesure prouve la non-régression sur le corpus versionné ; elle ne remplace
ni un corpus externe aveugle ni un pilote avec de vrais messages de chat.

## Comparaison avec des embeddings locaux

Le banc d’essai isolé `benchmarks/vector-comparison` compare le cœur à
`intfloat/multilingual-e5-small` via FastEmbed 5.17.4 et ONNX.
Le modèle MIT, multilingue et de 384 dimensions, est fingerprinté depuis son
cache local ; ni ses dépendances ni ses 487 352 545 octets n’entrent dans le
workspace, le SBOM ou l’application portable par défaut.

Mesure Windows x64 du 1er août 2026, sur les mêmes 84 titres et 328 annotations :

| Moteur | Seuil accepter/revoir | Précision | Rappel | Exactitude | Faux positifs |
|---|---:|---:|---:|---:|---:|
| lexical | politique v1 | 100 % | 100 % | 100 % | 0 |
| vecteurs, défaut | 0,90 / 0,82 | 93,13 % | 98,91 % | 86,89 % | 20 |
| vecteurs, calibré | 0,93 / 0,89 | 97,76 % | 95,62 % | 90,85 % | 6 |

| Coût local | p50 | p95 | p99 |
|---|---:|---:|---:|
| lexical | 7,6 µs | 17,8 µs | 33,0 µs |
| inférence vectorielle, cible unique | 8,17 ms | 12,17 ms | 14,42 ms |
| inférence vectorielle, 84 cibles | 7,91 ms | 12,78 ms | 13,77 ms |

Le chargement depuis un cache déjà présent a pris 20,67 s ; la construction de
l’index versionné de 165 expressions, 333 ms et 780 595 octets. Un corpus global
dédié de 40 cas est ensuite opposé aux 84 titres : 20 réponses positives, 6
ambiguïtés et 14 rejets hors catalogue ou bruit de chat. Le lexical y atteint
100 % de précision/rappel mais 87,5 % d'exactitude, car il rejette cinq cas dont
l'attendu prudent est l'abstention. Le vectoriel calibré (`0,93 / 0,89 / marge
0,00`) atteint 95 % de précision/rappel, 92,5 % d'exactitude, une fausse
acceptation, cinq abstentions et quinze rejets. Les 20 bons titres sont classés
premiers, mais un positif reste sous le seuil d'acceptation.

La calibration
vectorielle franchit le gate minimal de 95 %/90 %, mais reste moins précise,
moins fidèle et environ mille fois plus lente au p50 que le lexical. Les vecteurs
restent donc **désactivés par défaut** et ne sont pas distribués dans la portable.

Cette conclusion vaut pour les réponses courtes, alias et fautes du corpus v2.
Un futur corpus aveugle de paraphrases réelles pourra produire un résultat
différent ; la couture `semantic-engine-vectors` permet de le tester sans changer
le cœur ni rendre le modèle obligatoire. La baseline machine-readable conserve
le fingerprint exact du modèle et les mesures utilisées pour cette décision.

## Garde-fous

- Les clés sont des SHA-256 en mémoire ; aucun texte brut supplémentaire n’est
  persisté par le cache.
- Une policy non finie ou une entrée invalide continue de produire le verdict
  contractuel du cœur au lieu de casser la génération de clé.
- La capacité zéro désactive explicitement le cache pour comparer les chemins.
- Compter les hits ne suffit pas : suivre p50/p95/p99, taux de conflit, taille du
  contexte et pression d’écriture de l’audit.
- Un index vectoriel importé est lié à la version du contexte et au fingerprint
  du modèle ; dimensions, nombres non finis, doublons et vecteurs nuls sont refusés.
