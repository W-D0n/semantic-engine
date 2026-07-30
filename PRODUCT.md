# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

L’utilisateur prioritaire est l’animateur ou développeur d’un jeu live de type
« guess the movie/game ». Les participants répondent rapidement dans un chat et
l’opérateur doit détecter la première réponse juste malgré fautes et raccourcis.

## Product Purpose

Valider à faible latence qu’un message signifie la réponse attendue. Le moteur
retourne une décision expliquée et réutilisable ; un workflow externe attribue
les points, verrouille le round et gère le scoreboard.

## Positioning

Un moteur local-first spécialisé dans les réponses courtes et les titres,
explicable, prudent et portable, plutôt qu’un chatbot ou un LLM distant généraliste.

## Operating Context

Application Tauri portable pendant un live Twitch, utilisable seule ou pilotée
par une webapp telle que `C:\DEV\vault-workspace\myvault`. Les messages arrivent
en rafale ; l’ordre, la déduplication et l’idempotence sont critiques.

## Capabilities and Constraints

- Tolérer casse, accents, ponctuation, articles, nombres, acronymes et fautes.
- Accepter titres VO/VF et alias configurés.
- Répondre assez vite pour arbitrer le premier participant.
- Fonctionner hors ligne sans installation obligatoire ni compte cloud.
- Exposer un contrat stable à Tauri, HTTP/WebSocket et futurs microservices.
- Rust, Tauri et une évolution en services font partie de la cible.
- Scoreboard et règles de victoire restent des consommateurs externes.
- Choix de licence et stratégie exacte de modèle sémantique encore ouverts.

## Evidence on Hand

- Catalogue local de 1 207 jeux dans `myvault-games-import.json`.
- MyVault possède déjà SvelteKit, un EventBus typé, WebSocket, Twitch EventSub,
  workflows et une primitive Levenshtein.
- Aucun benchmark réel de messages Twitch n’est encore disponible.

## Product Principles

1. La première réponse juste doit être décidée vite et de façon reproductible.
2. Une réponse douteuse ne déclenche jamais silencieusement un point.
3. Le moteur valide ; le workflow orchestre.
4. Le local portable reste complet même si une offre service apparaît.
5. Les alias explicites gagnent sur l’illusion d’une compréhension universelle.

## Accessibility & Inclusion

L’interface opérateur doit rester utilisable au clavier, lisible pendant un live
et ne pas communiquer une décision uniquement par la couleur.
