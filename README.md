# zd-support

CLI Rust pour l’API Requests de Zendesk. Portail par défaut :
`https://support.outscale.com`.

## Installation

Les [releases GitHub](https://github.com/cleyrop/zd-support/releases) contiennent
les binaires macOS Apple Silicon/Intel et Linux ARM64/x86_64. Les binaires macOS
sont signés Developer ID et notarisés ; chaque archive est attestée par GitHub.
Linux est compilé sur Ubuntu 24.04 (glibc 2.39) ; utiliser une distribution compatible.
Windows n’est pas distribué ni validé pour cette première release.

Exemple pour macOS Apple Silicon, avec GitHub CLI installé :

```sh
gh release download v0.3.5 --repo cleyrop/zd-support \
  --pattern 'zd-support-v0.3.5-aarch64-apple-darwin.zip' --pattern SHA256SUMS
gh attestation verify zd-support-v0.3.5-aarch64-apple-darwin.zip --repo cleyrop/zd-support
unzip zd-support-v0.3.5-aarch64-apple-darwin.zip
mkdir -p "$HOME/.local/bin"
install -m 755 zd-support-v0.3.5-aarch64-apple-darwin/zd-support "$HOME/.local/bin/zd-support"
```

Ajouter `~/.local/bin` au `PATH` si nécessaire. Pour Intel, remplacer `aarch64`
par `x86_64`. Pour Linux, choisir l’archive `*-unknown-linux-gnu.tar.gz` puis
utiliser `tar -xzf ARCHIVE`. Les archives incluent les licences des dépendances
et les sources correspondantes des dépendances MPL.

Pour compiler depuis les sources : Rust 1.90.0, épinglé dans `rust-toolchain.toml`.
Un navigateur est nécessaire uniquement pour le mode OAuth facultatif.

```sh
cargo install --locked --path .
zd-support --help
```

Sur la machine de développement, le SDK macOS 27.2 sélectionné par défaut est
incompatible avec le linker installé. Le build est validé avec :

```sh
SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk \
  cargo install --locked --path .
```

## Connexion avec ton compte support

```sh
zd-support login
```

La CLI demande ton adresse e-mail du portail support, puis ton mot de passe
Zendesk avec une saisie masquée. Aucun navigateur, identifiant de client OAuth
ou sous-domaine Zendesk n’est nécessaire. Tu peux aussi préciser l’e-mail :

```sh
zd-support login --email 'toi@entreprise.fr'
zd-support whoami
zd-support list --json
```

Le portail par défaut est `https://support.outscale.com`. Il faut utiliser les
identifiants du **compte support Zendesk**, pas les clés API du compte cloud.
La CLI utilise HTTP Basic sur HTTPS. Elle vérifie l’identité et l’accès en lecture
à Requests avant d’enregistrer les identifiants ; aucun ticket n’est créé.
Un échec conserve les identifiants précédemment enregistrés.

**Condition côté OUTSCALE :** l’accès API par mot de passe doit être autorisé pour
les utilisateurs finaux. Un 401 peut indiquer des identifiants refusés ou cette
option désactivée ; un 403 peut aussi indiquer un e-mail non vérifié ou des droits
insuffisants. La CLI ne peut pas déterminer la cause exacte à partir de ce seul
code. Un compte connecté uniquement par SSO n’a pas forcément de mot de passe
Zendesk utilisable par cette méthode.

Pour une automatisation, un gestionnaire de secrets peut fournir le mot de passe
sur stdin avec `login --email ... --password-stdin`. Le mot de passe n’est jamais
accepté comme argument de commande. La saisie interactive reste le mode par défaut.

## OAuth facultatif

L’ancien mode reste disponible avec une commande distincte :

```sh
zd-support login-oauth --client-id IDENTIFIANT_FOURNI \
  --zendesk-subdomain SOUS_DOMAINE_FOURNI
```

Ce mode nécessite un client OAuth **public** fourni par OUTSCALE, PKCE S256,
les scopes `read write` et l’URL de retour autorisée exactement
`http://127.0.0.1:8765/callback`. Il ouvre le navigateur par défaut et renouvelle
les jetons. Il n’est pas requis pour `zd-support login`.

Variables communes : `ZD_SUPPORT_URL`, `ZD_SUPPORT_STATE_DIR`, `ZD_SUPPORT_EMAIL`.
Variables OAuth : `ZD_SUPPORT_CLIENT_ID`, `ZD_SUPPORT_SUBDOMAIN`,
`ZD_SUPPORT_REDIRECT_URI`, `ZD_SUPPORT_SCOPE`.

## Commandes

```sh
zd-support list                         # tableau, sans les tickets closed
zd-support list --include-closed        # tous les statuts
zd-support list --status closed         # uniquement les tickets closed
zd-support list --status open --all --json
zd-support show 12345                   # fiche du ticket avec statut coloré
zd-support show 12345 --json
zd-support comments 12345               # conversation avec auteurs et couleurs
zd-support comments 12345 --json
zd-support comments 12345 --color never # sans couleurs
zd-support comments 12345 --color always | less -R

# Ces commandes envoient immédiatement le message.
zd-support create --subject '[eu-west-2] Incident réseau' --body-file incident.txt
zd-support reply 12345 --body 'Voici les résultats du diagnostic.'
printf '%s\n' 'Diagnostic complémentaire…' | zd-support reply 12345 --body-file -

zd-support solve 12345
zd-support watch 12345 --interval 60 --json
zd-support logout
```

`--body` et `--body-file` sont exclusifs ; les messages vides sont refusés.
La CLI refuse les messages de plus de 64 000 octets UTF-8 avant tout envoi,
pour éviter la troncature silencieuse documentée par Zendesk au-delà de 64 kB.
Cette limite compte les octets : un caractère accentué peut en utiliser plusieurs.
Scinder les diagnostics longs en plusieurs réponses.
`solve` vérifie `can_be_solved_by_me`. `logout` efface les identifiants locaux sans les
révoquer côté Zendesk ni déconnecter le navigateur.

`show` affiche une fiche encadrée : numéro, statut coloré, sujet, dates UTC,
priorité et type lorsqu’ils sont disponibles, puis description initiale avec
mise en évidence du code. Les priorités `high` et `urgent` apparaissent en rouge.
Un rappel de la commande `comments` permet de retrouver tous les échanges.

`comments` affiche chaque message dans un cadre, avec le nom de son auteur,
la date en UTC et son numéro dans la conversation. Tes messages sont en cyan
(`VOUS`), ceux des agents Zendesk en vert (`SUPPORT`) et ceux des autres
participants en magenta. Le dernier message est signalé. Le texte s’adapte à
la largeur du terminal, le code est en jaune et les métadonnées sont atténuées.
Les noms et rôles viennent de l’API ; un auteur inconnu conserve son identifiant.
Les échanges restent dans l’ordre chronologique, du plus ancien au plus récent.

Pour `show` et `comments`, les couleurs sont automatiques dans un terminal et désactivées dans un pipe,
un fichier, avec `TERM=dumb` ou une variable `NO_COLOR` non vide.
`--color always` et `--color never` permettent de forcer ce choix.
`--json` reste sans codes couleur et conserve les messages originaux.

Pour les tickets OUTSCALE, inclure région, compte cloud, horodatage UTC,
ressources concernées et diagnostic. Les pièces jointes ne sont pas implémentées.

## Utilisation par un agent

- `--json` produit du JSON sur stdout et conserve les diagnostics sur stderr.
  `watch --json` émet du NDJSON au démarrage, puis à chaque changement de ticket.
  Utiliser `comments` pour lire la conversation publique.
- `list` masque les tickets `closed` par défaut, y compris en JSON ; `solved`
  reste inclus. Le filtre est appliqué par l’API avant la pagination.
  `--include-closed` affiche tous les statuts et `--status closed` uniquement
  les tickets fermés. Ces deux options sont exclusives.
- L’affichage texte de `list` utilise un tableau adapté à la largeur du terminal
  avec retour à la ligne des sujets longs. `--json` conserve le format JSON.
- La liste est triée par date de mise à jour décroissante, directement par l’API
  avant pagination : les tickets modifiés le plus récemment apparaissent en premier.
- `list` retourne les 50 premiers résultats filtrés et `next_page` ; `--all`
  suit toutes les pages sans modifier le filtre. `comments` récupère toutes les pages.
- Codes de sortie : `0` succès, `1` erreur d’exécution, `2` erreur de syntaxe.
- En mode OAuth, les jetons expirants sont renouvelés avant les appels API, puis sauvegardés
  atomiquement. Un GET recevant 401 peut être renouvelé et rejoué une fois.
  Les écritures ne sont jamais rejouées automatiquement.
- En cas d’erreur réseau lors d’une écriture, vérifier le ticket avant de
  renvoyer : le serveur peut avoir accepté le message malgré l’erreur locale.
  Même précaution pour une erreur serveur HTTP 5xx ou une réponse inattendue.
  Un HTTP 2xx sans identifiant de ticket valide n’est pas considéré comme un succès.
- Une sortie interrompue volontairement par un lecteur tel que `head` termine
  proprement la CLI. Si l’affichage échoue autrement après une écriture acceptée,
  le diagnostic précise de ne pas renvoyer le message.
- En mode OAuth, un refresh token expiré ou refusé nécessite `login-oauth`.
  Un HTTP 403 peut signaler des droits/scopes insuffisants ou un e-mail non vérifié.
  Un 429 indique le `Retry-After` reçu.
- Un verrou empêche la rotation concurrente des jetons. `watch` libère ce verrou
  entre les sondages afin que les autres commandes puissent agir.
- Les redirections HTTP et les liens de pagination vers une autre origine sont
  refusés ; seuls Requests et Users/Me sont utilisés pour les appels métier.

## Stockage

Le mot de passe (ou les jetons en mode OAuth) est stocké **en clair localement**, dans un fichier
`0600` et un répertoire `0700` sous Unix. Ces secrets ne sont jamais affichés.
Sous Unix, les fichiers de session/verrou trop permissifs et les liens symboliques
sur ces fichiers ou sur le répertoire du portail sont refusés. Les liens éventuels
sur les répertoires parents restent autorisés. Le stockage n’utilise pas encore le
trousseau système : réserver ce mode à un compte système de confiance. Les ACL
Windows et les configurations Unix avec ACL supplémentaires ne sont pas validées.

Sur macOS :
`~/Library/Application Support/zd-support/support.outscale.com_443/session.json`.
Sous Linux : répertoire de données XDG de `zd-support`.

`--state-dir /chemin/prive` permet de choisir un autre répertoire ; les sessions
sont séparées par hôte et port. Les anciennes sessions OAuth restent lisibles. Les anciennes sessions par cookies
de `outscale-support` ne sont pas migrées : relancer `zd-support login`.

## Développement et validation

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo deny --locked check advisories sources
cargo package --locked --offline --allow-dirty
```

Les tests utilisent un faux serveur local. Ils couvrent HTTP Basic, la vérification
de Requests avant sauvegarde, la conservation de la session en cas de refus,
les permissions du fichier, OAuth PKCE, la rotation des jetons et la pagination.
Ils couvrent aussi les payloads des commandes `create`, `reply`, `solve`,
le refus de résolution sans permission, les réponses API incohérentes,
les erreurs HTTP non JSON, les messages trop longs et les fichiers liés/trop permissifs.
Aucun ticket réel n’est créé par les tests. La connexion par mot de passe et
la lecture des conversations ont également été vérifiées sur OUTSCALE.

Les workflows GitHub exécutent les tests sur les quatre plateformes distribuées.
La procédure de publication est décrite dans [CONTRIBUTING.md](CONTRIBUTING.md).

## Références

- [Zendesk : authentification des utilisateurs finaux](https://developer.zendesk.com/api-reference/introduction/security-and-auth/#end-user-authentication)

- [Zendesk : OAuth et configuration du client](https://support.zendesk.com/hc/en-us/articles/4408845965210-Using-OAuth-authentication-with-your-application)
- [Zendesk : PKCE](https://developer.zendesk.com/documentation/authentication/oauth-pkce/)
- [Zendesk : refresh tokens](https://developer.zendesk.com/documentation/authentication/refresh-token/)
- [Zendesk : Requests API](https://developer.zendesk.com/api-reference/ticketing/tickets/ticket-requests/)
- [Zendesk : limites de taille des commentaires](https://developer.zendesk.com/api-reference/ticketing/tickets/ticket_comments/#body-size-limits)
