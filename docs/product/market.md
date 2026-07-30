# Ouvertures produit et marché

## Point d’entrée

> Reconnaissance fiable de réponses et commandes courtes pour expériences
> interactives en direct, local-first et explicable.

Ce segment donne des messages réels, une boucle de correction rapide et une
valeur visible. Éviter le positionnement flou de « moteur NLP universel ».

| Extension | Valeur | Risque |
|---|---|---|
| quiz, votes et jeux live | démonstration immédiate | pics de charge |
| support communautaire | réduit le tri | données sensibles |
| voix transcrite | même moteur | erreurs cumulées |
| modération assistée | marché large | safety/légal élevés |
| multilingue et edge | portée et confidentialité | calibration/packaging |

## Commercialisation

1. **Apache-2.0 + offre managée** : adoption forte, mais moteur hébergeable par un tiers.
2. **Open core** : moteur ouvert ; SSO, équipe, audit et HA commerciaux.
3. **AGPL + licence commerciale** : défendabilité accrue, adoption entreprise réduite.
4. **Runtime gratuit + services à l’usage** : cohérent seulement si le local reste complet.

Décider Apache-2.0 ou AGPL avant le premier code externe. Dans tous les cas,
format, export, CLI et moteur restent utilisables sans compte payant.

## Hypothèses et expériences

- Les créateurs perdent assez de valeur à cause des fautes pour installer l’outil.
- L’abstention expliquée est préférable à un LLM généraliste.
- Local-first constitue une raison d’achat.
- Twitch est un meilleur beachhead que YouTube ou un webhook générique.

Tester avec un quiz de 100 messages bruités, 8–12 entretiens, une démonstration
avant/après, un simulateur sans OAuth et un pilote réel.

Mesurer temps au premier résultat, rétention 7/30 jours, corrections par mille
messages, connexions actives, conversion équipe/managé et coût de support.
