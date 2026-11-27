---
title: "Instalação"
description: "Binários pré-compilados, Nix e NixOS, compilação a partir do código-fonte e decodificação de vídeo por hardware."
group: "Start"
order: 1
source: "README.md"
source_hash: "cfa1d6d72d2d2d70ec490beee7a7427dc539ecc108e3fc98a3c83628a166e5f3"
---

O veil precisa de um compositor Wayland com `wlr-layer-shell` e do GStreamer 1.x, com os conjuntos de plugins `base`, `good`, `bad` e `libav`, em tempo de execução (os decodificadores por hardware estão em `bad`). `XDG_RUNTIME_DIR` deve estar definido, como em qualquer sessão de desktop.

## Binários pré-compilados

Cada [lançamento](https://github.com/ordreth/veil/releases) tem um tarball para Linux x86_64 e outro para aarch64, com o binário, a unidade de usuário do systemd, as páginas de manual, o autocompletar do shell, os documentos e um script de instalação, além do `SHA256SUMS`:

```bash
version=1.0.0    # the release you want
tarball=veil-$version-$(uname -m)-unknown-linux-gnu.tar.gz
curl -LO https://github.com/ordreth/veil/releases/download/v$version/$tarball
curl -LO https://github.com/ordreth/veil/releases/download/v$version/SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf "$tarball" && cd "${tarball%.tar.gz}"
sudo ./install.sh                  # into /usr/local
./install.sh --prefix ~/.local     # or for your user only, without root
```

`./install.sh --help` lista o que vai para onde, e `--uninstall` (com o mesmo `--prefix`) remove tudo de novo. Cada tarball também tem uma atestação de proveniência de compilação, que diz que o workflow de lançamento do GitHub o compilou a partir do commit da tag: `gh attestation verify "$tarball" --repo ordreth/veil`. Depois, [inicie o daemon](/apps/veil/docs/usage#iniciar-o-daemon) ou [a unidade do systemd](/apps/veil/docs/usage#executar-como-serviço-de-usuário-do-systemd), que o script instala com o caminho do binário. Para `~/.local`, `~/.local/bin` precisa estar no seu `PATH`, e o zsh precisa da linha de `fpath` que o script imprime; bash, fish, nushell, `man` e systemd procuram lá por conta própria. Iniciado à mão, dê ao daemon `MALLOC_ARENA_MAX=1`, como a unidade faz (como em `MALLOC_ARENA_MAX=1 veil daemon`), ou sua memória cresce a cada vídeo para o qual ele troca.

O binário usa o GStreamer, a GLib e a biblioteca cliente do Wayland do sistema, e exige glibc 2.34 ou mais recente. Instale os plugins do GStreamer (sem arquivos de desenvolvimento, sem Rust):

```bash
sudo apt install libwayland-client0 gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad gstreamer1.0-libav                      # Debian, Ubuntu
sudo dnf install libwayland-client gstreamer1-plugins-base gstreamer1-plugins-good \
  gstreamer1-plugins-bad-free gstreamer1-plugin-libav gstreamer1-plugin-openh264   # Fedora
sudo pacman -S --needed wayland gstreamer gst-plugins-base gst-plugins-good \
  gst-plugins-bad gst-libav                                        # Arch Linux
```

O tarball x86_64 foi instalado com esses pacotes e executado em um sway headless no Debian 12 (GStreamer 1.22.0), Debian 13 (1.26.2), Ubuntu 22.04 (1.20.3), Ubuntu 24.04 (1.24.2), Fedora 44 e Arch Linux (ambos 1.28.8), em 2026-10-19: uma imagem, um MP4 H.264 e um WebM VP9 foram exibidos e reproduzidos em todos eles. GIFs animados exigem o GStreamer 1.26 ou mais recente: no Ubuntu 22.04 e 24.04 e no Debian 12 eles ficam parados ou não iniciam (no Debian 12, também com um núcleo de CPU inteiro ocupado), por causa do demuxer de GIF do GStreamer; [converta-os para vídeo](/apps/veil/docs/troubleshooting#um-gif-animado-fica-parado-não-inicia-ou-mantém-um-núcleo-ocupado). No Ubuntu 24.04, os vídeos de 1080p exibiram apenas de metade a um quarto dos seus quadros no sway headless, por um motivo ainda não encontrado ([problema conhecido 8](/apps/veil/docs/known-issues#8-versões-mais-antigas-do-gstreamer-gifs-animados-e-quadros-descartados-no-ubuntu-2404)). O workflow de lançamento compila o tarball aarch64 com o mesmo script em uma máquina ARM e executa os testes lá; ele não foi executado com um compositor. O workflow `Installation on distributions` repete a verificação do x86_64 toda semana, com os pacotes acima ([`docs/testing.md`](https://github.com/ordreth/veil/blob/stable/docs/testing.md)).

## Nix e NixOS

```bash
nix run github:ordreth/veil/stable -- daemon     # try it without installing
nix profile add github:ordreth/veil/stable       # install it (`nix profile install` in older Nix)
```

Em uma configuração do NixOS, pelo overlay do flake:

```nix
# flake inputs: veil.url = "github:ordreth/veil/stable";
nixpkgs.overlays = [ veil.overlays.default ];
environment.systemPackages = [ pkgs.veil ];
# The systemd user unit, started with the graphical session:
systemd.packages = [ pkgs.veil ];
systemd.user.services.veil.wantedBy = [ "graphical-session.target" ];
```

Por causa da licença, o nixpkgs considera o veil unfree. Os pacotes do próprio flake já a permitem; com o overlay, permita-a na sua configuração: `nixpkgs.config.allowUnfreePredicate = pkg: builtins.elem (lib.getName pkg) [ "veil" ];`.

O pacote é um wrapper em torno do binário que traz seu próprio conjunto de plugins do GStreamer e um cache de plugins do GStreamer próprio (`~/.cache/veil/gstreamer-registry.bin`), e define `MALLOC_ARENA_MAX=1`, a menos que você o defina. Ele inclui a unidade de usuário do systemd, o autocompletar do shell e as páginas de manual. A decodificação por hardware usa os drivers do sistema (`hardware.graphics`, e `hardware.nvidia` para NVIDIA); com o Nix em outra distribuição, isso não foi testado.

`stable` é o lançamento mais recente. Sem ele (`github:ordreth/veil`) você recebe o `main`, o trabalho rumo ao próximo lançamento, cuja versão termina em `-dev` ([Versions and branches](https://github.com/ordreth/veil/tree/stable#versions-and-branches)).

## A partir do código-fonte

São necessários Rust 1.89 ou mais recente, um compilador C e `pkg-config`, os arquivos de desenvolvimento do Wayland, do `libxkbcommon` e do GStreamer, e os plugins do GStreamer. Estes são os pacotes, verificados em 2026-10-19: o veil foi compilado com eles em um contêiner de cada distribuição, e o resultado exibiu uma imagem e reproduziu um vídeo H.264, um vídeo VP9 e um GIF animado em um sway headless. Outras versões dessas distribuições não foram testadas.

**Arch Linux** (GStreamer 1.28.8, Rust 1.99):

```bash
sudo pacman -S --needed base-devel git rust pkgconf wayland libxkbcommon \
  gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-libav
sudo pacman -S gst-plugin-va    # VA-API hardware decoding (Intel, AMD)
```

**Debian 13** (GStreamer 1.26.2). O Rust do Debian (1.85) é antigo demais, então o Rust vem do [rustup](https://rustup.rs):

```bash
sudo apt install build-essential git pkg-config curl libwayland-dev libxkbcommon-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"    # or open a new shell
```

**Ubuntu 26.04** (GStreamer 1.28.2, Rust 1.93):

```bash
sudo apt install build-essential git pkg-config cargo libwayland-dev libxkbcommon-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav
```

**Fedora 44** (GStreamer 1.28.8, Rust 1.99):

```bash
sudo dnf install gcc git rust cargo pkgconf-pkg-config wayland-devel libxkbcommon-devel \
  gstreamer1-devel gstreamer1-plugins-base-devel gstreamer1-plugins-good \
  gstreamer1-plugins-bad-free gstreamer1-plugin-libav gstreamer1-plugin-openh264
```

O GStreamer próprio do Fedora não tem decodificador por software para HEVC e decodifica H.264 com `openh264dec` (do repositório da Cisco, que o Fedora habilita). Para HEVC por software, o FFmpeg completo do RPM Fusion é o complemento usual (não testado aqui). No Debian, no Ubuntu e no Fedora, os decodificadores VA-API e NVIDIA estão nos plugins `bad`; o Arch tem o VA-API em `gst-plugin-va`. A decodificação por hardware não foi testada nos contêineres (eles não tinham GPU).

Depois, compile o veil e instale o binário, a unidade do systemd e as páginas de manual (aqui para o seu usuário; ajuste os caminhos para uma instalação em todo o sistema):

```bash
git clone --branch stable https://github.com/ordreth/veil && cd veil
cargo build --release --locked
install -Dm755 target/release/veil ~/.local/bin/veil
mkdir -p ~/.config/systemd/user ~/.local/share/man/man1
sed "s|/usr/bin/veil|$HOME/.local/bin/veil|" contrib/systemd/veil.service > ~/.config/systemd/user/veil.service
~/.local/bin/veil man-pages ~/.local/share/man/man1
```

`~/.local/bin` precisa estar no seu `PATH`. O autocompletar está em [Autocompletar do shell e páginas de manual](/apps/veil/docs/usage#autocompletar-do-shell-e-páginas-de-manual). Iniciado à mão, em vez de pela unidade, dê a um binário compilado por você `MALLOC_ARENA_MAX=1` (como em `MALLOC_ARENA_MAX=1 veil daemon`), ou sua memória cresce a cada vídeo para o qual ele troca.

## Atualização

O veil não se atualiza sozinho e não procura novas versões. Um novo lançamento é anunciado na [página de lançamentos](https://github.com/ordreth/veil/releases) (o GitHub pode avisar você: **Watch**, **Custom**, **Releases**), e os papéis de parede salvos, o arquivo de configuração e os ajustes da unidade do systemd são mantidos nas atualizações. Como atualizar depende de como você o instalou:

- **Binários pré-compilados:** baixe o novo tarball e verifique-o como acima, depois execute o `./install.sh` dele com o mesmo `--prefix` de antes; ele substitui os arquivos antigos. Depois, `systemctl --user restart veil` (ou reinicie o `veil daemon`).
- **Entrada de flake do Nix:** `nix flake update veil` (ou `nix flake lock --update-input veil` em Nix mais antigo) na sua configuração, recompile, depois `systemctl --user restart veil`. **`nix profile`:** `nix profile upgrade veil` (`nix profile list` mostra o nome que ele tem no seu perfil). A nova versão chega quando estiver no `stable`.
- **A partir do código-fonte:** `git pull` no seu checkout (no `stable`), compile e instale de novo como acima, depois reinicie o serviço.

`veil --version` diz qual versão está em execução.

## Decodificação de vídeo por hardware

O GStreamer escolhe um decodificador por hardware quando algum funciona, e o veil registra no log qual deles obteve para cada vídeo (`video decoder: nvh264dec (hardware)`). Intel e AMD precisam de um driver VA-API (`intel-media-driver`, ou o Mesa para AMD) e do plugin `va` do GStreamer; NVIDIA precisa do driver proprietário, usado pelo `nvcodec` do GStreamer. Sem nenhum dos dois, os vídeos são decodificados por software e continuam sendo reproduzidos, com um custo maior de CPU. [`docs/troubleshooting.md`](/apps/veil/docs/troubleshooting#sem-decodificação-por-hardware) explica como verificar e como forçar a decodificação por software.
