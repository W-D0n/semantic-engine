# Intégration YouTube Live

Recherche effectuée le 1er août 2026 à partir des seules documentations
officielles Google et YouTube.

## Décision

L'adaptateur YouTube doit utiliser `liveChatMessages.streamList` en priorité.
Il s'agit d'un flux serveur gRPC chiffré, plus réactif et moins coûteux qu'une
boucle REST. `liveChatMessages.list` reste un repli de compatibilité, en
respectant strictement `pollingIntervalMillis`.

Pour l'application portable, le parcours principal est OAuth 2.0 **Desktop
app**, Authorization Code avec PKCE S256, navigateur système et redirection
loopback sur `127.0.0.1` avec un port aléatoire. Le seul scope demandé pour la
lecture et la découverte des lives de l'opérateur est
`https://www.googleapis.com/auth/youtube.readonly`. Google considère une
application installée comme incapable de garder un secret ; le
`client_secret` est facultatif à l'échange du code et ne doit pas constituer
une barrière de sécurité. La documentation recommande aussi un `state`
imprévisible contre le CSRF et interdit l'authentification dans une WebView
embarquée. [OAuth pour application de bureau](https://developers.google.com/youtube/v3/guides/auth/installed-apps)

Un mode sans OAuth est techniquement possible pour un live public connu : le
guide officiel accepte une clé API pour `videos.list` et `streamList`.
Cependant, une clé commune incluse dans un exécutable portable est extractible.
Google déconseille d'inclure une clé dans le code client et recommande de la
restreindre. Le produit local ne disposant pas d'un serveur de secrets, ce mode
doit utiliser une clé fournie par l'opérateur, limitée à la YouTube Data API,
stockée dans le coffre système. [Guide officiel du flux gRPC](https://developers.google.com/youtube/v3/live/streaming-live-chat),
[bonnes pratiques des clés API](https://docs.cloud.google.com/docs/authentication/api-keys-best-practices)

> **Gate de commercialisation :** les Developer Policies interdisent en termes
> larges de créer des données ou métriques dérivées à partir de données de
> l'API. Transformer un message de chat en validation puis en points peut entrer
> dans cette catégorie. L'exception publiée depuis juin 2026 vise des usages
> analytics acceptés après audit et ne confirme pas explicitement un jeu live.
> Il faut soumettre le cas exact — lecture du chat, reconnaissance de réponse,
> attribution temporaire de points et éventuel scoreboard — à un **YouTube API
> Compliance Audit** avant publication commerciale. Ce document est une analyse
> technique, pas un avis juridique. [Guide de conformité](https://developers.google.com/youtube/terms/developer-policies-guide),
> [politiques sur les métriques dérivées](https://developers.google.com/youtube/terms/derived-metrics-policy)

## Parcours d'autorisation portable

1. Enregistrer un client OAuth de type **Desktop app** dans un projet Google
   Cloud ayant YouTube Data API v3 activée.
2. Au clic « Connecter YouTube », générer un `code_verifier` de 43 à 128
   caractères, son challenge S256 et un `state` aléatoire à usage unique.
3. Écouter uniquement `http://127.0.0.1:{port-aléatoire}`. Ouvrir
   `https://accounts.google.com/o/oauth2/v2/auth` dans le navigateur système
   avec `response_type=code`, le redirect exact, le scope `youtube.readonly`,
   le challenge et le `state`.
4. Refuser une réponse dont le `state`, l'adresse locale ou la durée de vie ne
   correspondent pas à la tentative active. Échanger le code une seule fois
   auprès de `https://oauth2.googleapis.com/token`, avec le `code_verifier`.
5. Vérifier le champ `scope` réellement accordé. Enregistrer access token,
   refresh token et échéance dans le coffre d'identifiants de l'OS, jamais dans
   SQLite, les logs, l'export portable ou le frontend.
6. Rafraîchir avant expiration et remplacer atomiquement les jetons si Google
   en renvoie de nouveaux. Un `invalid_grant` impose une reconnexion, sans
   boucle de retry.
7. « Déconnecter YouTube » appelle
   `https://oauth2.googleapis.com/revoke`, puis supprime immédiatement les
   jetons locaux et lance la purge des données concernées.

Google recommande PKCE pour les applications de bureau, le stockage des jetons
dans un coffre adapté à la plateforme — Credential Locker sous Windows — et la
révocation dès qu'ils ne sont plus nécessaires. La documentation actuelle
indique qu'un refresh token est renvoyé aux applications installées, mais il
peut expirer ou être révoqué ; il faut donc traiter son absence ou son
invalidation comme un état normal. [Bonnes pratiques OAuth](https://developers.google.com/identity/protocols/oauth2/resources/best-practices),
[échange, rafraîchissement et révocation](https://developers.google.com/youtube/v3/guides/auth/installed-apps#token-revocation)

Le scope en lecture suffit à `liveBroadcasts.list`. Il ne faut pas demander
`youtube.force-ssl` ni `youtube` tant que l'adaptateur ne publie, ne modère et
ne supprime rien : YouTube exige le scope minimal réellement utilisé.
[Scopes de `liveBroadcasts.list`](https://developers.google.com/youtube/v3/live/docs/liveBroadcasts/list),
[politique de minimisation](https://developers.google.com/youtube/terms/developer-policies#D_Accessing_YouTube_API_Services)

Les service accounts ne sont pas une alternative générale : YouTube ne les
prend en charge que pour certains content owners qui administrent plusieurs
chaînes. [OAuth pour applications installées](https://developers.google.com/youtube/v3/guides/auth/installed-apps#callinganapi)

## Découverte du `liveChatId`

Deux entrées indépendantes doivent être proposées :

| Parcours | Appel | Résultat |
|---|---|---|
| live de la chaîne connectée | `liveBroadcasts.list?part=id,snippet,status&broadcastStatus=active&broadcastType=all` avec OAuth | `items[].snippet.liveChatId` |
| URL ou ID vidéo saisi | `videos.list?part=liveStreamingDetails&id={videoId}` avec OAuth ou clé API | `items[0].liveStreamingDetails.activeLiveChatId` |

`activeLiveChatId` n'existe que tant que la vidéo est réellement en direct et
que son chat est actif ; il disparaît lorsque la diffusion est terminée. Le
`snippet.liveChatId` d'une ressource `liveBroadcast` est l'identifiant attendu
par les méthodes de chat. [Ressource `video.liveStreamingDetails`](https://developers.google.com/youtube/v3/docs/videos#liveStreamingDetails),
[ressource `liveBroadcast`](https://developers.google.com/youtube/v3/live/docs/liveBroadcasts#snippet.liveChatId)

Le produit doit distinguer : aucun live actif, live sans chat, live terminé,
live inaccessible et plusieurs lives actifs. Il ne doit jamais choisir
silencieusement le premier élément en cas d'ambiguïté ; l'opérateur arbitre.

## Acquisition basse latence

### `streamList` — parcours nominal

`streamList` ouvre un canal TLS vers `youtube.googleapis.com:443` et invoque
`youtube.api.v3.V3DataLiveChatMessageService.StreamList`. Le protocole est du
**server-streaming gRPC** : chaque frame applicative décodée est un protobuf
`LiveChatMessageListResponse` complet, pas un fragment JSON ou une ligne JSON.
Elle contient notamment `items`, `next_page_token`, `offline_at`, `page_info`
et l'éventuel sondage actif. Les messages de chaque réponse sont ordonnés du
plus ancien au plus récent. [Guide et proto officiels](https://developers.google.com/youtube/v3/live/streaming-live-chat)

La première connexion fournit seulement un historique récent. Chaque réponse
porte un `nextPageToken`; après coupure, le client se reconnecte avec le dernier
token durablement reçu afin de reprendre au même point. L'adaptateur doit aussi
dédupliquer les messages, car une coupure entre réception et checkpoint peut
provoquer une relecture. [Référence `streamList`](https://developers.google.com/youtube/v3/live/docs/liveChatMessages/streamList)

Deux détails de contrat méritent un test dédié :

- le proto officiel précise que `max_results` **n'est pas utilisé** par le RPC
  streaming, même si le paramètre apparaît dans la référence générale ;
- le proto streaming ne contient pas `polling_interval_millis`. Cette valeur
  gouverne le repli REST `list`, pas le rythme d'un flux poussé.

Au branchement sur une session Semantic Engine, la première réponse historique
ne doit pas pouvoir gagner un round commencé plus tard. La source effectue une
phase de rattrapage, fixe un marqueur local, puis n'émet vers le moteur que les
messages postérieurs au début du round. Le `pageToken` est un curseur fournisseur
et ne remplace pas la séquence globale durable attribuée par le runtime commun.

### `list` — repli REST

`GET https://www.googleapis.com/youtube/v3/liveChat/messages` prend
`liveChatId`, `part=id,snippet,authorDetails`, un `pageToken` optionnel et
`maxResults` entre 200 et 2 000 (500 par défaut). Il renvoie les messages du plus
ancien au plus récent, un nouveau `nextPageToken` et
`pollingIntervalMillis`. Le client attend **au minimum** cette durée avant le
prochain appel ; interroger plus tôt produit `rateLimitExceeded`.
[Référence `liveChatMessages.list`](https://developers.google.com/youtube/v3/live/docs/liveChatMessages/list)

Chaque page supplémentaire consomme un nouvel appel. La page officielle de
quota attribue 1 unité à `videos.list`, indique qu'une requête invalide coûte au
moins 1 unité, que chaque page est facturée et qu'un projet dispose par défaut
de 10 000 unités quotidiennes pour les endpoints regroupés, remises à zéro à
minuit heure du Pacifique. Elle ne publie actuellement pas de coût distinct
pour `liveChatMessages.list` ou `streamList`; ne pas coder un coût supposé :
mesurer la consommation réelle dans la console et conserver un budget par
source. [Calculateur de quota](https://developers.google.com/youtube/v3/determine_quota_cost),
[coût de `videos.list`](https://developers.google.com/youtube/v3/docs/videos/list)

## Mapping des messages

Seuls les objets dont `snippet.type == "textMessageEvent"` deviennent des
réponses candidates. Les événements de modération, dons, membres, sondages,
tombstones et fin de chat restent des signaux de source ou sont ignorés ; ils
ne doivent jamais être interprétés comme du texte utilisateur.

| Champ YouTube | Champ source Semantic Engine | Règle |
|---|---|---|
| `liveChatMessage.id` | `provider_message_id` | identifiant d'idempotence, préfixé par la source et le chat |
| `snippet.textMessageDetails.messageText` | `raw_text` éphémère | borne stricte en octets/caractères avant normalisation |
| `snippet.authorChannelId` ou `authorDetails.channelId` | `provider_user_id` | identité stable ; ne pas utiliser `displayName` comme clé |
| `authorDetails.displayName` | `display_name` | présentation seulement, valeur changeante/non unique |
| `snippet.publishedAt` | `provider_timestamp` | ordre fournisseur et filtre de rattrapage, pas horloge de confiance |
| `snippet.liveChatId` | `provider_channel_id` | cloisonne idempotence et reprise |

YouTube attribue normalement un ID unique à chaque message. Exception : un
`giftEvent` peut réutiliser son ID pour mettre à jour un combo. Le MVP ne
transformant que les `textMessageEvent`, l'ID est stable dans le périmètre
traité ; un futur support des cadeaux devra gérer une mise à jour, pas jeter le
second événement comme doublon. [Schéma `liveChatMessage`](https://developers.google.com/youtube/v3/live/docs/liveChatMessages)

Le runtime doit borner la taille, supprimer caractères de contrôle non admis,
ne jamais charger les URLs d'avatar et ne jamais faire confiance au nom
d'affichage. Le texte brut n'entre ni dans les logs techniques ni dans l'audit
persistant du moteur.

## Erreurs, reprise et fin de source

| Signal | Décision |
|---|---|
| coupure TCP/TLS, timeout, HTTP `408`/`429`/`5xx`, statut gRPC transitoire | reconnexion bornée avec backoff exponentiel et jitter, puis reprise avec le dernier token |
| `401` ou access token expiré | une tentative de refresh, puis reprise ; si `invalid_grant`, état `authorization_required` |
| `RESOURCE_EXHAUSTED` / `rateLimitExceeded` | ne pas boucler ; augmenter le délai, respecter le rythme YouTube et exposer l'état `rate_limited` |
| `quotaExceeded` | arrêter les appels jusqu'au reset/relèvement de quota et notifier l'opérateur |
| `PERMISSION_DENIED` / `forbidden` | pas de retry automatique ; vérifier compte, scope et droits |
| `INVALID_ARGUMENT` / page token invalide | abandonner le curseur, signaler un gap observable et refaire une acquisition fraîche avec accord opérateur |
| `liveChatDisabled` | état terminal pour ce live |
| `liveChatEnded`, `chatEndedEvent` ou `offlineAt` | finir proprement la pompe et préserver la session moteur |
| `NOT_FOUND` / `liveChatNotFound` | redécouvrir une fois le live ; sinon état terminal |

La référence mappe explicitement `PERMISSION_DENIED`, `INVALID_ARGUMENT`,
`FAILED_PRECONDITION`, `NOT_FOUND` et `RESOURCE_EXHAUSTED` côté gRPC, ainsi que
`forbidden`, `liveChatDisabled`, `liveChatEnded`, `liveChatNotFound` et
`rateLimitExceeded` côté web. [Erreurs `streamList`](https://developers.google.com/youtube/v3/live/docs/liveChatMessages/streamList#errors),
[erreurs Live Streaming API](https://developers.google.com/youtube/v3/live/docs/errors#liveChatMessages.streamlist)

Les retries concernent seulement une lecture idempotente et doivent avoir une
durée totale maximale. Google recommande un backoff exponentiel avec jitter
pour les erreurs transitoires, et déconseille les retries immédiats, infinis ou
empilés à plusieurs niveaux. [Stratégie de retry Google Cloud](https://docs.cloud.google.com/storage/docs/retry-strategy)

## Conservation, suppression et conformité

La politique YouTube impose notamment :

- des conditions d'utilisation qui renvoient aux YouTube Terms of Service ;
- une politique de confidentialité accessible et acceptée, mentionnant
  YouTube API Services, la Google Privacy Policy, les données collectées,
  leurs usages/partages, la procédure de suppression et la page de révocation
  Google ;
- une commande claire de révocation ; la révocation initiée dans le produit
  doit appeler Google immédiatement et les Authorized Data doivent être
  supprimées au plus tard sous 7 jours ;
- la suppression des données utilisateur demandée au plus tard sous 7 jours ;
- la suppression ou le rafraîchissement sous 30 jours des autres Authorized
  Data et des données publiques/non autorisées temporairement conservées ;
- la vérification périodique des autorisations et la suppression des données
  quand un jeton ne peut plus être rafraîchi ;
- la capacité d'une application installée à recevoir des mises à jour lorsque
  l'API ou ses règles évoluent.

[YouTube API Services Developer Policies](https://developers.google.com/youtube/terms/developer-policies#A_API_Client_Terms_of_Use_and_Privacy_Policies),
[règles de stockage et suppression](https://developers.google.com/youtube/terms/developer-policies#E_Handling_YouTube_Data_and_Content)

Le profil recommandé est plus strict : message brut uniquement en mémoire le
temps de la validation, aucune URL d'avatar, pseudonyme technique minimisé,
curseur de reprise limité au live actif, purge automatique à la fin du délai
produit et bouton « Supprimer mes données YouTube ». Un export doit exclure les
jetons, clés, messages bruts et identifiants YouTube sauf nécessité validée.

Une purge doit être auditable sans recopier la donnée supprimée : catégorie,
source, date, motif, nombre d'éléments et résultat. La suppression locale doit
préciser qu'elle ne supprime rien sur YouTube.

### Risque spécifique au jeu et au scoreboard

Les règles interdisent de fusionner des API Data YouTube avec d'autres sources
et de produire des données dérivées hors exceptions acceptées. Un classement
commun Twitch + YouTube, une identité inter-plateformes, une validation
persistée ou un score calculé depuis les messages ne doivent donc pas être
commercialisés sur la seule base d'une interprétation interne. Le dossier
d'audit doit montrer :

1. le texte exact lu et sa durée de vie ;
2. la transformation en verdict et en points ;
3. qui voit les données et le classement ;
4. l'absence de profilage et de métrique sur la performance de la chaîne ;
5. la séparation des fournisseurs ;
6. les parcours de consentement, révocation, export et suppression.

Si YouTube refuse ce cas, l'adaptateur doit rester désactivé dans la distribution
publique. Il ne faut pas contourner l'API officielle par scraping ou protocole
non documenté : les Developer Policies l'interdisent explicitement.

## Contraintes de test et de publication

- Un projet OAuth en statut **Testing** accepte au plus 100 testeurs déclarés.
  Leur autorisation et leur refresh token expirent sept jours après le
  consentement pour ces scopes. Séparer projets de test et de production.
  [Audience OAuth et expiration en test](https://support.google.com/cloud/answer/15549945)
- Une application publique qui demande des scopes d'accès aux données doit
  préparer la vérification OAuth : identité/branding cohérents, domaine,
  politique de confidentialité, justification du scope minimal et vidéo de
  démonstration du parcours complet. La catégorie exacte du scope est affichée
  dans la console. [Gestion des scopes et démonstration](https://support.google.com/cloud/answer/15549135)
- Un canal de test doit être vérifié, autorisé au live et sans restriction de
  diffusion depuis 90 jours. Les lives « made for kids » n'ont pas de chat.
  [Conditions du live](https://support.google.com/youtube/answer/2474026),
  [restrictions du chat](https://support.google.com/youtube/answer/2853834)
- Utiliser un live **non répertorié** piloté par l'équipe pour le canary. La
  documentation officielle ne décrit pas de sandbox synthétique pour
  `streamList`; le test de bout en bout nécessite donc un vrai live actif et
  des comptes consentants. Les tests automatiques ordinaires doivent rejouer
  des protobufs de fixtures sans appeler Google. Cette absence de sandbox est
  une inférence prudente à partir des parcours de test officiels.
- Tester au minimum : chat désactivé, fin de live, historique initial, reprise
  sur token, token invalide, double livraison, panne réseau, refresh OAuth,
  révocation, quota épuisé, deux sources simultanées et message hostile à la
  taille limite.

Une extension de quota exige un API Compliance Audit ; le cas d'usage accordé
ne peut pas être réutilisé pour un autre produit sans nouvelle approbation.
[Quotas et audit](https://developers.google.com/youtube/terms/developer-policies#D_Accessing_YouTube_API_Services)

## Contrat recommandé pour l'adaptateur

L'adaptateur reste derrière la même interface que Twitch et ne connaît ni le
scoreboard ni l'algorithme de reconnaissance. Sa configuration publique expose
uniquement le mode d'identification (`connected_channel` ou `video_id`), l'ID
vidéo éventuellement choisi, l'état et une référence opaque de credential.

Il garantit :

- zéro secret dans la vue, l'API loopback, SQLite ou les exports ;
- `streamList` prioritaire, `list` explicitement identifié comme repli ;
- ordre fournisseur préservé, idempotence par message et gap visible ;
- historique initial neutralisé avant l'ouverture d'un round ;
- backpressure bornée vers le runtime ;
- arrêt propre sur fin de chat sans clôturer arbitrairement la session métier ;
- purge et révocation disponibles depuis l'UI comme depuis l'API locale ;
- feature flag de distribution tant que le Compliance Audit n'a pas validé
  l'usage de données dérivées.

Cette séparation maintient Semantic Engine autonome : YouTube est une source
d'entrée optionnelle, pas un composant du moteur ni l'autorité du workflow qui
consomme ses verdicts.
