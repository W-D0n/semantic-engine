---
status: proposed
---

# Séparer le moteur des adaptateurs de source

Le moteur expose uniquement reconnaissance et correction ; Twitch, YouTube,
terminal, HTTP, OAuth et affichage restent derrière des adaptateurs ou dans le
plan de contrôle. Ce coût de contrats explicites permet de tester sur corpus,
changer de plateforme et livrer une bibliothèque locale indépendante.
