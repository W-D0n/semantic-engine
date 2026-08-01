# Utiliser un pipeline hybride et abstentionniste

- Statut : accepté
- Date : 2026-08-01

## Décision

Combiner règles déterministes, expressions, fautes et embeddings locaux optionnels,
puis s’abstenir si score ou marge sont insuffisants. Un dictionnaire seul couvre
mal les paraphrases ; une approche LLM/vecteurs seule est moins prévisible,
explicable et économique. Les environnements contraints restent sans modèle.

Le lexical est le moteur par défaut. Les vecteurs vivent derrière une couture
modèle-agnostique et ne rejoignent une distribution qu’après un benchmark
versionné démontrant un gain sans baisse de précision. Un index est immuable et
lié à la version du contexte ainsi qu’au fingerprint du modèle.

## Preuve initiale

Sur le corpus v2 de 328 annotations, `multilingual-e5-small` calibré atteint
97,76 % de précision, 95,62 % de rappel et 90,85 % d’exactitude, avec 6 faux
positifs. Le lexical atteint 100 % sur les trois métriques, sans faux positif,
avec un p50 environ mille fois plus faible et sans modèle de 487 Mo. Les vecteurs
restent donc désactivés par défaut. Sur un corpus global dédié de 40 cas opposés
aux 84 titres, les vecteurs atteignent 95 % de précision/rappel et 92,5 %
d'exactitude contre 100 %/100 %/87,5 % pour le lexical : ils gèrent mieux certaines
abstentions, mais introduisent une fausse acceptation.

## Conséquences

- la portable et le serveur headless restent autonomes, légers et explicables ;
- le benchmark ONNX possède son propre lockfile et son cache hors distribution ;
- de futurs fournisseurs peuvent réutiliser la même interface ;
- un corpus aveugle de paraphrases est requis avant toute réévaluation produit.
