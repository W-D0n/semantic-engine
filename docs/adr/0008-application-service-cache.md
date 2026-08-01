# ADR-0008 — Service d’application et cache borné

- **Statut** : accepté
- **Date** : 2026-08-01

## Contexte

Tauri possédait un ledger éphémère tandis que l’audit était un autre état. Une
future CLI durable, l’API loopback et l’hôte headless auraient dû recopier cette
orchestration. Ajouter un cache directement au cœur aurait mélangé algorithme de
reconnaissance, durée de vie du processus et persistance.

## Décision

`semantic-engine-service` devient la façade d’application partagée. Il orchestre
validation, identité idempotente, résolution et audit. Son cache TTL-LRU est
strictement borné et indexé par le round complet, le texte exact et l’empreinte
optionnelle du paquet de contexte. Tauri appelle cette façade en mémoire.

La clé de déduplication inclut toute la soumission et reste seulement en mémoire.
Une reprise contradictoire échoue. Une capacité de cache nulle fournit le chemin
de comparaison sans cache. La CLI expose un benchmark reproductible plutôt que
des chiffres codés dans le produit.

## Conséquences

- Les futurs transports partagent la même sémantique sans dépendre de Tauri.
- Le cœur lexical reste pur, déterministe et testable sans horloge ni stockage.
- Le cache économise le calcul mais ne remplace pas l’audit ; son gain doit être
  mesuré sur le chemin complet.
- Les sessions et la liaison explicite d’une version de contexte restent le
  prochain ajout à cette façade.
