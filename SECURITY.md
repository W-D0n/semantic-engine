# Politique de sécurité

## Versions suivies

Avant la première release stable, seule la branche `master` et la dernière
préversion publiée reçoivent des correctifs de sécurité. Après `v1.0`, la matrice
des versions maintenues sera publiée ici.

## Signaler une vulnérabilité

Ne publiez pas de preuve d'exploitation, jeton, contenu de chat ou donnée
personnelle dans une issue publique. Utilisez **Security → Report a
vulnerability** dans le dépôt GitHub afin d'ouvrir un avis privé.

Le rapport devrait contenir : version/commit, système, préconditions, impact,
étapes minimales de reproduction et suggestion éventuelle. Un accusé de
réception est visé sous 5 jours ouvrés. La qualification, le calendrier de
correction et la divulgation coordonnée dépendent de la gravité et de la
disponibilité du mainteneur.

## Périmètre prioritaire

- fuite de jeton Twitch ou du secret Bearer loopback ;
- écoute réseau hors loopback ou contournement d'origine/authentification ;
- exécution de code ou traversée de chemin via un paquet de contexte ;
- persistance involontaire du texte du chat ;
- contournement de limites provoquant un déni de service durable ;
- confusion d'identité, d'ordre ou d'idempotence affectant un résultat live.

Les erreurs de reconnaissance sans impact de sécurité vont dans le tracker
public normal. Consultez également le [threat model](docs/product/threat-model.md).
