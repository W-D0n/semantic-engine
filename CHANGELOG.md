# Changelog

Les changements notables suivent [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/)
et les versions suivent SemVer.

## [Unreleased]

### Added

- moteur lexical explicable avec alias, acronymes, tolérance et abstention ;
- paquets de contexte immuables, import/export, atelier et audit minimisé ;
- sessions durables, JSONL, HTTP/WebSocket loopback et clients de conformité ;
- application Tauri portable hors ligne avec arbitrage opérateur ;
- Twitch Device Code Grant, coffre OS, EventSub et reconnexion ;
- YouTube OAuth Desktop PKCE, découverte/sélection de live et chat gRPC reprenable ;
- API publique de sources et ordre global durable multi-source ;
- corpus annoté et quality gate précision/rappel reproductible ;
- contrat source v2 avec faute typée et reçu de révocation/purge sans secret ;
- benchmark live reproductible, SBOM, CI et documentation de sécurité.

### Security

- entrées bornées et non fiables, secrets hors SQLite/UI/logs, bind loopback
  imposé, Bearer éphémère, origines exactes et backpressure.

[Unreleased]: https://github.com/W-D0n/semantic-engine/compare/v0.1.0...HEAD
