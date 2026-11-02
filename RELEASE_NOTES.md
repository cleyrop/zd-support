**zd-support 0.4.0** ajoute la consultation de version et les mises à jour autonomes, tout en réduisant la taille du binaire.

```sh
zd-support version
zd-support self-update --check
zd-support self-update
```

- Réduction locale mesurée sur macOS ARM64 : 4,42 → 2,47 Mo, soit 44,13 %, nouvelles commandes incluses.
- `version` fonctionne hors ligne ; les deux commandes acceptent `--json`.
- Mise à jour depuis la dernière release stable immuable : empreintes SHA-256 via HTTPS, extraction limitée au binaire attendu et remplacement atomique après validation.
- Signature Developer ID Cleyrop contrôlée sur macOS ; aucune dépendance à GitHub CLI.
- Aucun changement de session, de commandes support ou de comportement des proxies.

Les quatre plateformes restent disponibles. Les binaires macOS sont signés et notarisés ; les archives et SHA256SUMS disposent d’attestations GitHub. Le mécanisme autonome vérifie SHA-256 via HTTPS, pas les attestations Sigstore.

Depuis 0.3.6, installer cette release manuellement une première fois : la commande `self-update` est nouvelle.
