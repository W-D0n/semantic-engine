# ADR 0010 — Coffre natif et Device Code Grant pour Twitch

## Statut

Accepté le 1er août 2026.

## Contexte

L’application portable ne peut pas protéger durablement un `client_secret`
embarqué : tout utilisateur du binaire pourrait l’extraire. Les jetons Twitch
sont révocables, rotatifs et ne doivent apparaître ni dans SQLite ni dans les
logs. Le produit doit rester utilisable sans installation et sans serveur OAuth
possédé par Semantic Engine.

## Décision

- enregistrer l’application Twitch comme client public ;
- utiliser le Device Code Grant et le seul scope `user:read:chat` ;
- stocker access/refresh tokens dans le coffre natif du système via une interface
  `CredentialVault` ;
- conserver dans `SourceStore` uniquement les paramètres publics et un
  `credential_id` opaque ;
- masquer les secrets dans `Debug`, borner leur taille et mettre leurs buffers à
  zéro à la libération ;
- valider le jeton au démarrage et toutes les 55 minutes, renouveler avant
  expiration et arrêter la source après révocation ;
- supprimer le jeton lors de la suppression de la source.

## Conséquences

Le dossier reste portable mais l’autorisation ne voyage pas entre machines, ce
qui est une propriété de sécurité. Windows, macOS et Linux délèguent la protection
au mécanisme de session du système. Un environnement Linux sans Secret Service
peut utiliser le moteur hors ligne mais doit fournir un coffre compatible avant
d’activer Twitch.

Le Client ID est public et peut être saisi dans la version développeur. Une
distribution commerciale pourra embarquer son propre Client ID public sans
changer le protocole ni introduire de secret applicatif.
