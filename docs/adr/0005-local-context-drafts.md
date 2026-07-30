# ADR 0005 — Calques locaux de contexte et décision opérateur

- Statut : accepté
- Date : 2026-07-30

## Contexte

Un opérateur de live doit corriger rapidement un titre ou un alias et trancher
une validation, sans corrompre un paquet public ni faire croire qu’une correction
éphémère constitue déjà un apprentissage ou un audit.

## Décision

1. Un paquet de contexte activé reste immuable.
2. Un réglage est stocké comme brouillon local SQLite, indexé par
   `(package_sha256, target_id)`.
3. La recherche fusionne le paquet actif et ses brouillons dans une lecture
   groupée, avec un nombre de résultats borné.
4. Une décision opérateur référence la validation et le round, conserve la
   décision moteur originale et ne peut accepter qu’une cible du round.
5. La décision opérateur reste éphémère dans ce premier incrément. Sa persistance
   exigera un schéma d’audit, une rétention et une politique de données explicites.
6. Diffuser un réglage créera une nouvelle version de paquet ; aucune release
   existante n’est réécrite.

## Conséquences

- l’opérateur peut régler l’application pendant un live et restaurer le publié ;
- un paquet diffusé reste vérifiable et reproductible ;
- les workflows de score peuvent consommer une résolution sans perdre la preuve moteur ;
- les brouillons ne voyagent pas avec le dossier portable tant qu’ils ne sont pas exportés ;
- l’UI doit distinguer clairement résultat moteur, brouillon local et version publiée.