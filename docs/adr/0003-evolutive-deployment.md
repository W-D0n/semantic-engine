# ADR 0003 — Déploiement évolutif, Rust et microservices

- Statut : accepté
- Date : 2026-07-30

## Contexte

Le produit doit d’abord être facile à lancer en local et réactif pendant un
live. Il doit aussi pouvoir être appelé par MyVault, évoluer vers plusieurs
sources et soutenir une offre hébergée. Rust et les microservices ne sont donc
pas hors périmètre.

## Décision

Le cœur est une bibliothèque Rust sans connaissance de Tauri, Twitch ou du
scoreboard. Trois façades partagent les mêmes types `Round`, `Submission` et
`Validation` :

1. appel embarqué dans Tauri ;
2. sidecar JSONL pour MyVault et l’automatisation locale ;
3. service réseau futur lorsque le déploiement l’exige.

Le premier livrable est un **monolithe modulaire déployé localement**, pas parce
que les microservices seraient indésirables, mais parce qu’un processus unique
réduit latence, installation et surface d’attaque. Le contrat rend ensuite
l’extraction d’un service réversible.

## Déclencheurs d’extraction

Un microservice devient justifié si au moins une condition mesurée apparaît :

- plusieurs produits doivent partager un moteur central mis à jour séparément ;
- les modèles vectoriels nécessitent un autre profil CPU/GPU ou mémoire ;
- l’isolation de données ou la montée en charge impose des workers indépendants ;
- une offre SaaS requiert quotas, observabilité et déploiement progressif.

## Conséquences

- Rust est le runtime principal du moteur et des façades natives.
- Tauri appelle la bibliothèque en mémoire, sans HTTP loopback.
- MyVault peut utiliser le sidecar dès maintenant, puis une API sans changer son
  modèle d’événement.
- Le service futur doit préserver les schémas versionnés et l’idempotence.
- La frontière réseau ajoutera authentification, rate limiting, TLS, audit et
  politiques de rétention ; elle ne sera pas simulée prématurément.

## Alternatives

- Tout TypeScript : intégration web simple, mais portabilité et réutilisation
  native moins fortes pour ce produit.
- Microservices dès M0 : faisable, mais coûts d’exploitation sans preuve de
  besoin.
- Réécriture Rust ultérieure : évitée puisque le cœur commence directement en
  Rust.
