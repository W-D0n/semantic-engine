# Comparaison vectorielle locale

Ce banc d’essai mesure `intfloat/multilingual-e5-small` contre le moteur lexical
sur exactement les mêmes titres et annotations. Il est volontairement isolé du
workspace Rust principal : son lockfile, ONNX et le modèle ne sont jamais requis
pour compiler ou lancer Semantic Engine.

```powershell
cargo run --locked --release `
  --manifest-path benchmarks/vector-comparison/Cargo.toml -- `
  --cache-dir artifacts/vector-benchmark/model-cache `
  --output artifacts/vector-benchmark/report.json `
  --index-output artifacts/vector-benchmark/index.json
```

Le premier lancement télécharge le modèle dans le cache indiqué. Les suivants
fonctionnent avec ce cache local. Le rapport contient le fingerprint SHA-256 de
tous les blobs du cache, la version du contexte, la taille de l’index, les
métriques de décision et les latences p50/p95/p99. Les liens internes créés par
Hugging Face ne sont acceptés que si leur cible canonique reste dans le cache.

Le benchmark balaie les seuils par pas de `0,01` et la marge d'ambiguïté de
`0,00` à `0,10`. Il mesure les 328 annotations mono-cible, puis un corpus global
dédié de 40 annotations contre les 84 cibles simultanément afin d'observer le top
1, le second score, les faux positifs et l'abstention sans réinterpréter un négatif
mono-cible. Cette calibration sur les mêmes corpus sert
à comparer des techniques, pas à prétendre à une généralisation : un seuil
destiné au produit devra être confirmé sur un corpus aveugle de messages réels.
La baseline Windows versionnée est dans [`baselines/`](baselines/).
