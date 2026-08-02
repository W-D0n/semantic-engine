# ADR-0011 — Distribuer les index de contextes avec TUF

- Statut : accepté
- Date : 2026-08-02

## Contexte

SHA-256 détecte une modification, mais n’authentifie pas l’éditeur et ne protège
pas seul contre rollback, gel, incohérence de métadonnées ou rotation de clés. Le
catalogue doit rester statique, portable et utilisable par d’autres moteurs.

## Décision

Les canaux utilisent TUF v1 complet : `root`, `targets`, `snapshot` et `timestamp`.
Le profil de canal et le registre monotone de révocations sont des cibles signées.
Le profil énumère explicitement les archives et embarque leurs métadonnées produit;
TUF épingle leurs octets et peut les résoudre dans un rôle délégué.

Le client Rust utilise une implémentation TUF conforme, une racine approuvée hors
bande et un cache durable. La première approbation est une action opérateur. Les
révocations déjà observées persistent comme tombstones.

Answer Atlas prépare des cibles déterministes compatibles avec le rôle `targets`
par défaut de TUF-on-CI. La signature de production reste une cérémonie humaine :
aucune clé privée n’est générée ou stockée par le build.

## Conséquences

- l’hébergement peut être GitHub Pages, un stockage objet ou un miroir statique ;
- GitHub compromis ne suffit pas à signer une fausse version ;
- les clients anciens peuvent mettre à jour leur racine par rotations séquentielles ;
- chaque éditeur peut déléguer un espace sans partager sa clé ;
- disponibilité réseau et identité de domaine restent distinctes de la confiance TUF ;
- publier exige une gestion explicite des clés, expirations et cérémonies.

## Alternatives

Une signature JCS ad hoc, un manifeste uniquement hashé, Sigstore seul et une API
centrale ont été écartés. Ils réimplémentent partiellement TUF, couvrent moins
d’attaques ou créent un couplage de service inutile.
