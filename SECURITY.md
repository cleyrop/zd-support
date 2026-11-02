# Sécurité

Signaler une vulnérabilité via l’onglet Security du dépôt, avec le signalement
privé GitHub. Ne pas publier de mot de passe, de jeton ou de contenu de ticket
dans une issue publique. Ne pas joindre de fichier `session.json`.

La version publiée la plus récente est prise en charge. Les correctifs sont
distribués dans une nouvelle version ; les anciens actifs ne sont pas remplacés.

## Limites

- Les identifiants Zendesk sont conservés en clair localement, protégés par
  les permissions Unix. Aucun trousseau système n’est utilisé pour cette session.
- Un processus du même compte utilisateur peut lire ces identifiants.
- Les fichiers locaux, sauvegardes et sorties JSON doivent être traités comme
  des données potentiellement confidentielles.
- Les écritures ne sont jamais réessayées automatiquement : après une réponse
  incertaine, vérifier la conversation avant de renvoyer le message.
- Seuls macOS et Linux sont distribués et testés. Les ACL additionnelles et les
  environnements multiutilisateurs nécessitent une revue spécifique.

Avant installation, vérifier les archives avec `gh attestation verify ARCHIVE
--repo cleyrop/zd-support` et contrôler les sommes publiées dans SHA256SUMS.
