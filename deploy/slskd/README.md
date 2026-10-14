# slskd on this Linux machine

The automated setup has now completed on this machine. Its owner-only
`data/credentials.json` contains the generated Soulseek login, local web login,
and API key. Burrata stores its API key in the OS credential store. The service
restarts automatically unless explicitly stopped. Do not publish the data folder.

Docker and Docker Compose are installed here. This runs the official slskd image
with the web/API port restricted to localhost. Soulseek's peer TCP port is 50300.
Burrata does not manage this service. The Compose configuration allows 32 download
slots because remote queue waits consume slskd slots. Burrata separately allows
32 pending Soulseek monitors and two local processing/direct-download workers.
This does not cap active peer transfers at two; slskd owns transport and retries.

From this directory:

```sh
mkdir -p data/downloads data/incomplete
cp slskd.example.yml data/slskd.yml
chmod 700 data
chmod 600 data/slskd.yml
printf 'SLSKD_UID=%s\nSLSKD_GID=%s\n' "$(id -u)" "$(id -g)" > .env
chmod 600 .env
```

Edit `data/slskd.yml` locally. Enter your Soulseek username/password, a unique web
password, and a randomly generated API key (at least 16 characters, up to 255).
Do not paste these credentials into chat. Keep the `${BATCH_ID}` destination so
Burrata can reliably identify each completed batch. No folders are shared by
this template; add explicit host volume mounts and share paths if desired.

```sh
docker compose up -d
docker compose logs --tail=30
```

Open <http://localhost:5030>, log in with the web credentials you chose, and check
that Soulseek connects. If incoming connections fail, forwarding TCP 50300 through
your router/firewall may be needed; outgoing searches alone do not verify this.

In Burrata → Settings → Sources & artist discovery → Soulseek:

- Address: `http://localhost:5030`
- API key: the `burrata` key you put in the YAML
- Completed-download folder: `/home/loki/projects/burrata/deploy/slskd/data/downloads`

Enable Soulseek, save, and test connections. Then choose Soulseek as your preferred
download source. To stop the service: `docker compose down` (keeps its data).

Official references: [Docker setup](https://github.com/slskd/slskd/blob/master/docs/docker.md)
and [configuration example](https://github.com/slskd/slskd/blob/master/config/slskd.example.yml).

## Sharing controls in Burrata

Settings → Sources & artist discovery → Soulseek includes current shares and an
explicit rescan. For this repository's managed local service only, it also offers
folder add/remove controls. Adding a folder mounts it read-only into the container,
updates `shares.directories`, and recreates the managed service; this may interrupt
Soulseek transfers. Files in shared folders are visible to other Soulseek users.
No folder is shared automatically. Filesystem roots and folders containing the
managed private configuration are refused. Removing a share leaves its music
files intact. Docker must be available to the desktop user; on this machine its
user has Docker-group access. Configuration edits are bounded, backed up with
private permissions, and rolled back if applying the service change fails.

A remote/custom service has read/rescan controls only; its operator must edit its
own configuration. Burrata does not enable slskd remote configuration.
