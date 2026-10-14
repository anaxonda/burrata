# Optional slskd setup for Soulseek

This directory contains an example Docker Compose setup for running slskd on
your computer. Burrata does not install or supervise slskd; it connects to the
service's local HTTP API. Docker and Docker Compose must be installed separately.

From this directory, create the local data folders and configuration:

```sh
mkdir -p data/downloads data/incomplete
cp slskd.example.yml data/slskd.yml
chmod 700 data
chmod 600 data/slskd.yml
printf 'SLSKD_UID=%s\nSLSKD_GID=%s\n' "$(id -u)" "$(id -g)" > .env
chmod 600 .env
```

Edit `data/slskd.yml` locally. Set your Soulseek username and password, a unique
web password, and a randomly generated API key (at least 16 characters, up to
255). Do not paste credentials into chat or commit the local data files. Keep the
`${BATCH_ID}` destination so Burrata can identify completed batches. The example
does not share any folders; add explicit host volume mounts and share paths only
if you choose to share files.

Start the service and check its logs:

```sh
docker compose up -d
docker compose logs --tail=30
```

Open <http://localhost:5030>, sign in with the web credentials you chose, and
check that Soulseek connects. If incoming connections fail, forwarding TCP 50300
through your router or firewall may be needed; outgoing searches alone do not
verify incoming connectivity.

In Burrata → Settings → Sources & artist discovery → Soulseek, enter
`http://localhost:5030`, the `burrata` API key from the YAML, and the full local
path to the `data/downloads` folder created above. Enable Soulseek, save, and test
the connection. Then choose Soulseek as your preferred download source. Stop the
service with `docker compose down`; its data remains on disk.

Official references: [Docker setup](https://github.com/slskd/slskd/blob/master/docs/docker.md)
and [configuration example](https://github.com/slskd/slskd/blob/master/config/slskd.example.yml).

## Sharing controls in Burrata

Settings → Sources & artist discovery → Soulseek lists current shares and offers
an explicit rescan. For Burrata's managed local service, it also offers folder
add/remove controls. Adding a folder mounts it read-only into the container,
updates `shares.directories`, and recreates the managed service; this can interrupt
Soulseek transfers. Files in shared folders are visible to other Soulseek users.
No folder is shared automatically. Filesystem roots and folders containing the
managed private configuration are refused. Removing a share leaves its music
files intact. Docker must be available to the desktop user. Configuration edits
are bounded, backed up with private permissions, and rolled back if applying the
service change fails.

A remote or custom service has read/rescan controls only; its operator must edit
its own configuration. Burrata does not enable remote slskd configuration.
