# Sécurité, safety et cadre légal

> Checklist produit, pas avis juridique. Faire valider usages, pays, plateformes
> et durées de conservation avant commercialisation.

Tout message, webhook, fichier, correction, plateforme et modèle est non fiable.
Un message ne devient jamais une commande système.

```mermaid
flowchart LR
    NET["Internet non fiable"] --> IN["Validation + quotas"] --> CORE["Moteur sans shell"]
    OP["Opérateur authentifié"] --> CP["Plan de contrôle"] --> CORE
    CP --> VAULT["Secrets isolés"]
    CORE --> DB["Données minimisées"]
    CORE --> OUT["Résultat sans action autonome"]
```

## Contrôles prioritaires

| Menace | Contrôle |
|---|---|
| taille, Unicode, fréquence | limites, timeout, file bornée, backpressure |
| webhook faux ou rejoué | signature, timestamp, nonce, idempotence |
| token OAuth volé | hors logs, coffre, rotation et révocation |
| accès à un autre contexte | autorisation par objet, identifiants opaques |
| mémoire empoisonnée | correction opérateur, revue, test, rollback |
| fuite par télémétrie | aucun contenu par défaut, redaction structurée |
| SSRF lors d’un ajout de source | schémas/hôtes autorisés, contrôle IP |
| dépendance ou modèle compromis | lockfile, SBOM, hash et provenance |
| décision à fort impact | abstention et confirmation humaine |

Tester l’API contre l’[OWASP API Security Top 10](https://owasp.org/www-project-api-security/).

## Auth

En local : loopback par défaut, secret généré, tokens dans le trousseau système,
action explicite pour exposer le réseau. En équipe : OpenID Connect, rôles
`viewer/operator/owner`, OAuth plateforme séparé, scopes minimaux et journal des
changements sans contenu intégral du chat.

## Safety

Une faible confiance produit une abstention. Les sanctions, paiements, diagnostics,
profilages ou refus de droit exigent politique dédiée et validation humaine.
Prévoir sandbox, rollback et arrêt immédiat des sources.

## Données et droit

Messages, pseudonymes et identifiants peuvent être personnels. Le [RGPD article
5](https://eur-lex.europa.eu/eli/reg/2016/679/art_5/oj) exige notamment finalité,
minimisation, durée limitée, sécurité et responsabilité démontrable. Documenter
base légale, notice, durées, export/effacement, transferts et besoin d’AIPD.

Le produit applique déjà une première minimisation : l’audit local exclut le
texte du chat et l’expression correspondante, conserve les identifiants, scores
et verdicts pendant au plus 30 jours ou 10 000 validations, et offre une purge
totale confirmée. Cette valeur technique par défaut ne remplace pas le choix
d’une durée adaptée à la finalité ni les obligations de la plateforme. Voir
[Audit local et confidentialité](audit.md).

Twitch recommande [EventSub et Twitch API](https://dev.twitch.tv/docs/chat/) ;
les WebSockets conviennent au local et les scopes sont détaillés dans
[l’authentification chat](https://dev.twitch.tv/docs/chat/authenticating/).

YouTube propose `liveChatMessages.streamList` dans la [documentation Live
Chat](https://developers.google.com/youtube/v3/live/docs/liveChatMessages). Ses
[Developer Policies](https://developers.google.com/youtube/terms/developer-policies)
imposent transparence, contrôle utilisateur et des limites de stockage/rafraîchissement
pouvant atteindre 30 jours selon la donnée.

Évaluer l’[AI Act européen](https://eur-lex.europa.eu/eli/reg/2024/1689/oj) selon
les fonctions réelles. Vérifier aussi licence et redistribution des modèles,
choisir la licence du dépôt avant contribution externe et respecter les marques.

Avant release : threat model, tests d’abus, inventaire des données, suppression,
procédure d’incident, politiques plateforme à jour et avis juridique ciblé.
