# veil: roteiro de teste de instalação (Arch, Fedora, NixOS)

Objetivo: descobrir se uma pessoa que nunca viu o projeto consegue instalar o veil seguindo o README, em sistemas limpos. O que se testa aqui é a **instalação** (tarball + `install.sh`, unit do systemd, flake do Nix), não o desempenho de vídeo.

Faça as partes **na ordem**. Cada passo é feito logo depois do anterior. Onde um passo diz "anote", escreva no seu caderno de testes (um arquivo `notas.md` qualquer) o que viu, mesmo que tenha funcionado.

Regra de ouro durante todo o teste: **siga o README ao pé da letra** e só use o que está escrito nele. Se você precisar fazer algo que o README não diz (instalar um pacote, exportar uma variável, reiniciar), isso é um achado: anote exatamente o que foi preciso.

Índice:

- Parte 0: preparar o host (uma vez)
- Parte 1: o roteiro padrão de instalação (é reutilizado em Arch e Fedora)
- Parte 2: Arch Linux (VM + roteiro padrão)
- Parte 3: Fedora (VM + roteiro padrão)
- Parte 4: NixOS limpo (VM + os 3 métodos)
- Parte 5: o caminho real do release (tag, GitHub, download)
- Parte 6: o que levar de volta para mim se algo falhar

---

## Parte 0: preparar o host (uma vez)

Tudo aqui roda na sua máquina, em terminais normais (fora do `devbox` se o `docker` não aparecer lá).

### 0.1 Conferir a virtualização por hardware (KVM)

```bash
ls -l /dev/kvm
id | tr ',' '\n' | grep -i kvm
```

Esperado: `/dev/kvm` existe e seu usuário está no grupo `kvm` (ou o dispositivo é acessível). Sem KVM a VM funciona, mas fica muito lenta (e compilar o veil no NixOS ficará inviável). Se não existir, veja se a virtualização está ligada na BIOS (VT-x/AMD-V).

### 0.2 Ferramentas, sem instalar nada no sistema

Estas ferramentas vêm de um `nix shell` temporário: nada é instalado no sistema.

```bash
nix shell nixpkgs#qemu nixpkgs#python3 nixpkgs#ffmpeg nixpkgs#coreutils
```

Esse shell precisa estar aberto em **todo terminal** onde você for rodar `qemu-system-x86_64`, `qemu-img`, `ffmpeg` ou o servidor HTTP. Se fechar, reabra com o mesmo comando (já baixado, abre rápido).

### 0.3 Pastas

```bash
mkdir -p ~/vms/iso ~/vms/disks ~/vms/share
```

- `~/vms/iso`: ISOs baixadas.
- `~/vms/disks`: discos das VMs (arquivos `.qcow2`).
- `~/vms/share`: **a única pasta que as VMs enxergam** (tarball, mídia de teste).

### 0.4 O script que liga as VMs

Crie `~/vms/run-vm.sh`:

```bash
#!/usr/bin/env bash
# uso: run-vm.sh <disco.qcow2> [iso]
#   RAM=8G CPUS=4 GL=0 ./run-vm.sh disco.qcow2   (variáveis opcionais)
set -eu
disk=$1
iso=${2:-}
args=(-enable-kvm -cpu host -smp "${CPUS:-4}" -m "${RAM:-4G}"
  -drive "file=$disk,if=virtio,format=qcow2"
  -nic user,hostfwd=tcp::2222-:22)
if [ "${GL:-1}" = 1 ]; then
  args+=(-device virtio-vga-gl -display gtk,gl=on)
else
  args+=(-device virtio-vga -display gtk)
fi
[ -n "$iso" ] && args+=(-cdrom "$iso" -boot d)
exec qemu-system-x86_64 "${args[@]}"
```

```bash
chmod +x ~/vms/run-vm.sh
```

O que ele faz:

- `-enable-kvm -cpu host`: velocidade quase nativa.
- `-nic user,hostfwd=tcp::2222-:22`: rede por NAT (a VM acessa a internet) e a porta 2222 do host vai para a porta 22 da VM (usaremos SSH opcionalmente).
- `virtio-vga-gl` com `gl=on`: dá à VM uma GPU virtual com aceleração. O sway precisa de um dispositivo gráfico DRM funcional.
- `GL=0`: modo de reserva sem OpenGL. Use se a janela da VM ficar preta, o QEMU reclamar de GL, ou o sway não iniciar. Nesse caso, dentro da VM inicie o sway com `WLR_RENDERER=pixman sway` (renderização por CPU, a mesma usada nos seus testes headless).
- O endereço `10.0.2.2` **dentro da VM** significa "o host". É assim que a VM baixa o tarball.

Dicas de uso da janela do QEMU: **Ctrl+Alt+G** solta o mouse da janela; **Ctrl+Alt+F** alterna tela cheia.

### 0.5 Gerar o tarball localmente (o mesmo que o workflow de release gera)

Isto usa o Docker do host. Fica na pasta do repositório:

```bash
cd ~/projects/ordreth/veil
grep -m1 '^version' Cargo.toml          # confirme a versão (esperado: 1.0.0-beta.1)
scripts/release/build.sh --out ~/vms/share
```

- Roda `cargo test` e `cargo build --release` num contêiner Debian 12 (a primeira vez baixa a imagem do Rust e demora vários minutos). `--no-tests` pula os testes se você só quer repetir o build rápido.
- Inclui mudanças **não commitadas** no working tree, mas só de arquivos que o git já rastreia. Se você criou um arquivo novo, `git add` não é necessário para o build, mas ele só entra se estiver rastreado (o script usa `git ls-files`). Confirme que o que quer testar está rastreado.

Ao fim:

```bash
ls -l ~/vms/share
```

Esperado: um arquivo `veil-1.0.0-beta.1-x86_64-unknown-linux-gnu.tar.gz`. Anote o nome exato; ele aparece em todos os comandos abaixo.

### 0.6 Simular o `SHA256SUMS` do release

O README manda conferir o hash com `sha256sum --check --ignore-missing SHA256SUMS`. O workflow cria o arquivo assim; faça igual:

```bash
cd ~/vms/share
sha256sum -- *.tar.gz > SHA256SUMS
cat SHA256SUMS
```

### 0.7 Mídia para testar (imagem e vídeos)

```bash
cd ~/vms/share
# uma imagem PNG 1920x1080
ffmpeg -y -f lavfi -i testsrc2=size=1920x1080 -frames:v 1 test.png
# um vídeo H.264 de 10 s, 1080p30 (o caso principal do README)
ffmpeg -y -f lavfi -i testsrc2=duration=10:size=1920x1080:rate=30 \
  -c:v libx264 -pix_fmt yuv420p test-h264.mp4
# um vídeo VP9 em WebM
ffmpeg -y -f lavfi -i testsrc2=duration=10:size=1280x720:rate=30 \
  -c:v libvpx-vp9 -pix_fmt yuv420p test-vp9.webm
ls -l
```

### 0.8 Servidor HTTP para as VMs baixarem os arquivos

Em **um terminal separado**, deixe aberto durante todos os testes:

```bash
cd ~/vms/share
python3 -m http.server 8000 --bind 127.0.0.1
```

Dentro de qualquer VM, `http://10.0.2.2:8000/` é essa pasta. Teste mais tarde com `curl -I http://10.0.2.2:8000/SHA256SUMS`. (O QEMU em modo "user" leva `10.0.2.2` para o `127.0.0.1` do host, por isso o `--bind 127.0.0.1` basta.) Nunca sirva uma pasta que não seja `~/vms/share`.

### 0.9 Baixar as ISOs e verificar

Coloque em `~/vms/iso` e **confira o checksum** publicado ao lado de cada uma:

| Sistema | Onde baixar | O que escolher |
|---|---|---|
| Arch Linux | https://archlinux.org/download/ (espelho do Brasil) | `archlinux-x86_64.iso` (a mais recente) e o `sha256sums.txt` do mesmo diretório |
| Fedora | https://fedoraproject.org/spins/sway/ | **Fedora Sway Spin**, Live ISO x86_64 (já vem com sway configurado, poupa muito trabalho). Use a mesma versão do README (Fedora 44) se disponível, e o arquivo `CHECKSUM` da página |
| NixOS | https://nixos.org/download/ | **Graphical ISO** (GNOME ou Plasma, 64-bit Intel/AMD) da versão estável mais recente. O instalador gráfico simula o que um usuário comum faz |

Verificação de exemplo:

```bash
cd ~/vms/iso
sha256sum -c --ignore-missing sha256sums.txt     # Arch
sha256sum <arquivo>.iso                          # compare a mão com o publicado (Fedora, NixOS)
```

### 0.10 Dica que economiza horas: digitar pelo seu terminal, não pelo console da VM

Digitar na janela do QEMU é lento e sem copiar/colar. Em cada VM, depois de instalada, habilite o SSH (instruções em cada parte). Daí:

```bash
ssh -p 2222 SEU_USUARIO@127.0.0.1
```

Atenção: **o veil precisa ser testado dentro da sessão gráfica do sway**, não numa sessão SSH solta, porque ele precisa de `WAYLAND_DISPLAY`. Use o SSH para copiar/colar comandos e logs, e a janela da VM para ver o resultado. Quando um comando precisar do ambiente do sway, rode-o em um terminal (`foot`) dentro do sway.

Para o teste da unit do systemd o SSH é traiçoeiro (a sessão SSH tem outro ambiente). Para esses passos, use o terminal do sway dentro da VM.

---

## Parte 1: o roteiro padrão de instalação (S1 a S12)

Este roteiro é executado **dentro de uma VM (Arch ou Fedora), como usuário comum, dentro da sessão do sway**, num terminal `foot` (atalho padrão para abrir: `Mod+Enter`). Ele existe uma vez só aqui; as Partes 2 e 3 dizem quando executá-lo.

Variáveis que serão usadas (defina no terminal da VM):

```bash
version=1.0.0-beta.1
tarball=veil-$version-$(uname -m)-unknown-linux-gnu.tar.gz
base=http://10.0.2.2:8000          # no teste real do release (Parte 5) isto vira a URL do GitHub
```

### S1. Conferir o ambiente

```bash
echo "XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR  WAYLAND_DISPLAY=$WAYLAND_DISPLAY  SWAYSOCK=$SWAYSOCK"
swaymsg -t get_outputs | grep -E '"name"|"scale"|current_mode' | head
systemctl --user show-environment | grep -E 'WAYLAND_DISPLAY|SWAYSOCK'
```

Anote:

- Se `WAYLAND_DISPLAY` e `SWAYSOCK` aparecem na **última** linha (o ambiente do systemd do usuário). Se aparecerem, a distro já exporta isso para o systemd. Se **não** aparecerem, o aviso da unit é real: um usuário novo vai precisar de `systemctl --user import-environment WAYLAND_DISPLAY SWAYSOCK` no config do sway. Isso decide o que o README precisa dizer.

### S2. Baixar e verificar o tarball (como o README manda)

```bash
mkdir -p ~/dl-veil && cd ~/dl-veil
curl -LO $base/$tarball
curl -LO $base/SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
```

Esperado: `OK`. Se a verificação disser `FAILED`, o hash do arquivo mudou (refaça o 0.6). Se disser que nenhum arquivo foi verificado, o nome do tarball não bate com o do `SHA256SUMS`.

### S3. Extrair e olhar o conteúdo

```bash
tar -xzf "$tarball" && cd "${tarball%.tar.gz}"
ls
./install.sh --help
```

Anote se a lista de arquivos bate com a do `--help`.

### S4. Antes de instalar as dependências: o que acontece num sistema sem elas?

```bash
ldd ./veil | grep 'not found'
./veil --version
```

- Se `ldd` mostrar `not found` (por exemplo `libgstreamer-1.0.so.0`), o veil não roda. **Anote a mensagem de erro exata**: é o que um usuário veria se pulasse a lista de dependências. Ela é compreensível? O README ajuda a resolver?
- Se tudo resolve (o spin do Fedora pode já trazer o GStreamer), ótimo, anote isso e siga em frente.

### S5. Instalar as dependências com o comando do README (copie do README, não daqui)

Abra o README (no tarball: `less README.md`, seção "Installation") e rode **o bloco da sua distribuição**. Para conferência, é este:

- Arch: `sudo pacman -S --needed wayland gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-libav`
- Fedora: `sudo dnf install libwayland-client gstreamer1-plugins-base gstreamer1-plugins-good gstreamer1-plugins-bad-free gstreamer1-plugin-libav gstreamer1-plugin-openh264`

Depois:

```bash
ldd ./veil | grep 'not found' || echo "todas as bibliotecas resolvem"
gst-inspect-1.0 waylandsink | head -3
gst-inspect-1.0 avdec_h264 | head -3
./veil --version
```

Anote:

- Se o `dnf`/`pacman` pediu algo a mais (confirmações, importar chave GPG).
- Se `waylandsink` e `avdec_h264` existem. No Fedora, `avdec_h264` pode **não** existir e o decodificador de H.264 vir do `openh264` (o README avisa). Veja com `gst-inspect-1.0 openh264dec | head -3`.

### S6. Instalar para o usuário (sem root): `~/.local`

```bash
./install.sh --prefix ~/.local
```

Leia a saída. Depois:

```bash
ls ~/.local/bin ~/.local/share/systemd/user ~/.local/share/man/man1 | head -30
cat ~/.local/share/systemd/user/veil.service | grep ExecStart
echo $PATH | tr ':' '\n' | grep -n local
command -v veil || echo "veil NÃO está no PATH"
```

Anote:

- Se `~/.local/bin` já está no `PATH`. No Arch normalmente **não** está; no Fedora costuma estar, mas só vale para shells abertos **depois** de a pasta existir. Se não estiver, o README diz isso ("`~/.local/bin` must be on your `PATH`")? Resolva do jeito que o README mandar (`export PATH=$HOME/.local/bin:$PATH` ou abrir um shell novo).
- O `ExecStart` deve ser `/home/SEU_USUARIO/.local/bin/veil daemon`.

Continue:

```bash
veil --version
man veil | head -5          # se 'man' não existir na distro, anote
```

E a completion do bash (só funciona se o pacote `bash-completion` existir): abra um shell novo e digite `veil ` seguido de TAB duas vezes. Anote se completou.

### S7. Rodar à mão, vendo o resultado na tela

Copie a mídia de teste:

```bash
cd ~ && curl -O $base/test.png -O $base/test-h264.mp4 -O $base/test-vp9.webm
```

Em um terminal (A), inicie o daemon:

```bash
MALLOC_ARENA_MAX=1 veil daemon
```

Em outro terminal (B) (`Mod+Enter` abre outro):

```bash
veil status
veil outputs
veil set --input ~/test.png      # a tela deve mostrar a imagem de teste (azul/colorida)
veil status
veil set --input ~/test-h264.mp4 # o vídeo deve rodar em loop
veil status
veil pause ; sleep 2 ; veil resume
veil set --input ~/test-vp9.webm
veil clear
veil quit
```

Anote, para cada comando, se a tela mudou e se o terminal A imprimiu algo relevante. Em especial: a linha `video decoder: ...` no log do terminal A (esperado nas VMs: um decodificador de **software** como `avdec_h264` ou `openh264dec`).

Em uma VM sem GPU real o vídeo pode engasgar; isso não é um achado do veil. O que importa é que **inicie, mostre e troque**.

### S8. Rodar pelo systemd (o caminho "oficial" para uso diário)

Estando no terminal do sway:

```bash
systemctl --user cat veil | head -5        # de onde veio a unit
systemctl --user daemon-reload
systemctl --user enable --now veil
systemctl --user status veil --no-pager
journalctl --user -u veil -b --no-pager | tail -20
veil status
```

Esperado: `active (running)`, o wallpaper de volta na tela (o salvo no S7, ou preto se você deu `clear`).

Se o `enable --now` falhar com algo como "Failed to connect to bus" ou o serviço sair com código 5, o veil não achou o compositor: volte ao S1 (variáveis no ambiente do systemd). Anote o texto do erro e o que resolveu.

Teste a recuperação de queda:

```bash
systemctl --user show veil -p MainPID
kill -9 $(systemctl --user show veil -p MainPID --value)
sleep 5
systemctl --user status veil --no-pager | head -5
veil status
```

Esperado: reiniciado sozinho em ~2–3 s, com o mesmo wallpaper.

### S9. Persistência e início com a sessão

```bash
veil set --input ~/test-h264.mp4
systemctl --user restart veil
sleep 3 ; veil status
```

Esperado: o vídeo volta sozinho (estado em `~/.local/state/veil/state.json`).

Agora a prova que importa para usuários reais:

1. Saia do sway: `swaymsg exit`.
2. No TTY (ou na tela de login), entre de novo e inicie o sway.
3. **Sem rodar nada**, veja se o wallpaper aparece e rode `systemctl --user status veil --no-pager`.

Se **não** subiu sozinho: a unit tem `WantedBy=graphical-session.target`, e **nem todo sway inicia o `graphical-session.target`**. Isso é um achado importante: um usuário que só faz `systemctl --user enable veil` ficaria sem wallpaper no login. Anote o que a sua distro faz e o que seria preciso (por exemplo, uma linha `exec systemctl --user start veil` no config do sway, como o comentário da unit sugere para o Hyprland). Depois de anotar, aplique a solução e confirme que funciona após novo login.

### S10. Desinstalar (usuário)

```bash
systemctl --user disable --now veil
cd ~/dl-veil/${tarball%.tar.gz}
./install.sh --prefix ~/.local --uninstall
find ~/.local ~/.config -iname '*veil*' 2>/dev/null
```

Esperado: nada sobra de instalação. O que **deve** continuar existindo é dado do usuário: `~/.local/state/veil/` e `~/.config/veil/` (se existir). Anote se a unit ficou "enabled" em `~/.config/systemd/user/graphical-session.target.wants/` (link quebrado). Se `systemctl --user disable` já foi feito antes de desinstalar, não deve sobrar.

### S11. Instalar para o sistema (root): `/usr/local`

```bash
cd ~/dl-veil/${tarball%.tar.gz}
sudo ./install.sh
command -v veil                    # esperado: /usr/local/bin/veil
systemctl --user cat veil | head -3
```

Atenção: `/usr/local/bin` precisa estar no `PATH` de usuário (quase sempre está). Repita **S8** (enable/status/restart). A pergunta específica deste passo: o systemd de usuário encontra uma unit em `/usr/local/share/systemd/user/`? Se `systemctl --user cat veil` disser "No files found", é um achado: o README promete que `sudo ./install.sh` instala a unit e ela não é encontrada nessa distro.

Depois desinstale:

```bash
systemctl --user disable --now veil
sudo ./install.sh --uninstall
find /usr/local -iname '*veil*'
```

### S12. Fechar o teste

- Passo opcional: `./install.sh --prefix ~/.local` de novo e confira que reinstalar por cima funciona (o script copia para arquivo temporário e renomeia, então um daemon rodando não deve quebrar).
- Desligue a VM (`systemctl poweroff`) e **anote o resultado final** (o que passou, o que foi achado).
- Reverta a VM ao snapshot `base` (veja a Parte 2/3) antes do próximo ciclo de teste, para sempre partir de um sistema limpo.

---

## Parte 2: Arch Linux

### 2.1 Criar o disco e iniciar o instalador

```bash
cd ~/vms
qemu-img create -f qcow2 disks/arch.qcow2 25G
RAM=4G CPUS=4 ./run-vm.sh disks/arch.qcow2 iso/archlinux-x86_64.iso
```

Na janela do QEMU: o menu do Arch aparece (modo BIOS); aperte Enter em "Arch Linux install medium". Você cai num shell de root.

### 2.2 Teclado e rede

```bash
loadkeys br-abnt2        # teclado brasileiro, se for o seu
ping -c2 archlinux.org   # a rede NAT já vem configurada por DHCP
```

### 2.3 Instalar com `archinstall`

```bash
archinstall
```

(Os nomes dos menus mudam um pouco entre versões do archinstall; siga a ideia.)

| Item | O que escolher |
|---|---|
| Idioma / Mirrors | Brasil (opcional, só acelera) |
| Disk configuration | "Best-effort default layout", disco `/dev/vda`, sistema de arquivos **ext4** |
| Bootloader | **Grub** (a VM está em modo BIOS, systemd-boot só serve para UEFI) |
| Hostname | `arch-veil` |
| Root password | defina uma |
| User account | crie seu usuário, marque **sudo** |
| Profile → Type | **Desktop → Sway** |
| Seat access (se perguntar) | `polkit` |
| Graphics driver | "All open-source" |
| Greeter (se perguntar) | nenhum (você entra pelo TTY e roda `sway`) |
| Audio | nenhum ou pipewire (tanto faz) |
| Network configuration | **NetworkManager** |
| Additional packages | `foot git curl openssh bash-completion man-db` |
| Timezone | América/São Paulo |

Escolha "Install". Ao terminar, responda para **reboot**; quando a VM reiniciar no instalador, **feche a janela**, e ligue de novo **sem a ISO**:

```bash
RAM=4G CPUS=4 ./run-vm.sh disks/arch.qcow2
```

Obs. dos pacotes extras: `openssh`, `bash-completion`, `man-db`, `git`, `curl` e `foot` **não são parte do que o README exige**, são do ambiente de teste (para você copiar/colar e testar `man` e a completion). Se `man-db` e `bash-completion` não estivessem instalados, `man veil` e a completion falhariam por motivos que não são do veil.

### 2.4 Preparar a base

Entre pelo TTY com seu usuário.

```bash
sudo systemctl enable --now sshd          # permite o ssh -p 2222 do host
```

Prepare o config do sway com Alt como tecla de atalho (o Super é capturado pelo seu host):

```bash
mkdir -p ~/.config/sway
cp /etc/sway/config ~/.config/sway/config
sed -i 's/^set \$mod Mod4/set $mod Mod1/' ~/.config/sway/config
# teclado brasileiro dentro do sway:
printf '\ninput * {\n    xkb_layout br\n}\n' >> ~/.config/sway/config
```

Inicie o sway:

```bash
sway
```

(Se a tela ficar preta ou o sway falhar com erro de renderer/DRM, saia, desligue a VM e religue com `GL=0 ./run-vm.sh ...`, e inicie com `WLR_RENDERER=pixman sway`. Se o cursor sumir: `WLR_NO_HARDWARE_CURSORS=1`.)

`Alt+Enter` abre o terminal. Confirme que funciona, saia do sway (`Alt+Shift+E` e confirme), e **desligue a VM**: `sudo systemctl poweroff`.

### 2.5 Snapshot do estado limpo

Com a VM **desligada**, no host:

```bash
cd ~/vms
qemu-img snapshot -c base disks/arch.qcow2
qemu-img snapshot -l disks/arch.qcow2
```

Para voltar ao estado limpo depois de cada ciclo de teste (VM desligada):

```bash
qemu-img snapshot -a base disks/arch.qcow2
```

### 2.6 Rodar o teste

Ligue a VM, entre pelo TTY, rode `sway` (com `WLR_RENDERER=pixman` se for o caso) e execute **a Parte 1 inteira (S1 a S12)**.

Pontos específicos do Arch para você olhar:

- Em S5, o `pacman` precisa de `-Sy`? O README usa `pacman -S --needed`. Se a base de pacotes estiver desatualizada, o `pacman` pode dar 404; num Arch recém-instalado isso raramente acontece, mas se acontecer, anote.
- No Arch, `~/.local/bin` normalmente **não** está no PATH (S6).
- Os pacotes do GStreamer vêm em `gst-plugins-bad` (inclui `waylandsink`), `gst-libav` (decodificadores). Confirme com `gst-inspect-1.0` (S5).
- O pacote `gst-plugin-va` (VA-API) não é necessário no teste.

Ao fim, desligue e restaure o snapshot `base` se for repetir.

---

## Parte 3: Fedora

### 3.1 Criar o disco e iniciar o instalador

```bash
cd ~/vms
qemu-img create -f qcow2 disks/fedora.qcow2 30G
RAM=4G CPUS=4 ./run-vm.sh disks/fedora.qcow2 iso/<a-iso-do-fedora-sway>.iso
```

Na tela de boot: "Start Fedora-Sway-Live". O sistema ao vivo carrega com o sway.

### 3.2 Instalar no disco

O Spin ao vivo traz uma opção de instalar (um atalho "Install to Hard Drive" no sway, ou um ícone/menu; procure na barra ou abra o terminal e rode `liveinst`). No instalador (Anaconda):

| Item | O que escolher |
|---|---|
| Idioma e teclado | Português (Brasil) / ABNT2 |
| Destino da instalação | o disco virtual de 30 GB, particionamento **automático** |
| Usuário | crie o seu usuário e marque "Tornar este usuário administrador" (isso dá `sudo`) |
| Root | pode deixar desabilitado |

Instale, depois desligue a VM (`sudo systemctl poweroff` no live), e ligue **sem a ISO**:

```bash
RAM=4G CPUS=4 ./run-vm.sh disks/fedora.qcow2
```

### 3.3 Preparar a base

A tela de login do Spin deve oferecer a sessão **Sway**. Entre nela.

Num terminal (`foot`):

```bash
sudo dnf install -y openssh-server git curl
sudo systemctl enable --now sshd
```

Se `Mod` (Super) for capturado pelo host, troque para Alt em `~/.config/sway/config` (ou, se não existir, copie `/etc/sway/config` para lá e altere a linha `set $mod Mod4` para `Mod1`).

Confirme o teclado e a rede. Desligue a VM e tire o snapshot:

```bash
cd ~/vms
qemu-img snapshot -c base disks/fedora.qcow2
```

### 3.4 Rodar o teste

Ligue a VM, entre na sessão Sway e execute **a Parte 1 inteira (S1 a S12)**.

Pontos específicos do Fedora para olhar:

- **S1**: o Fedora Sway Spin traz integração com o systemd do usuário; pode ser que `WAYLAND_DISPLAY` já esteja no ambiente e que o `graphical-session.target` já seja iniciado pelo sway. Se for assim, S9 provavelmente funciona sem nada extra. Anote.
- **S5**: o `dnf install` deve trazer `gstreamer1-plugin-openh264`; o H.264 será decodificado por `openh264dec`. Confirme com o log do daemon em S7 (`video decoder: openh264dec`).
- **S5**: se o Fedora Sway Spin já trazia alguns desses pacotes, o `dnf` dirá "Nothing to do" para eles; normal.
- **S6**: `~/.local/bin` costuma estar no PATH do Fedora **se a pasta já existia quando você logou**. Como `./install.sh --prefix ~/.local` pode ter criado a pasta agora, abra um shell **novo de login** (saia e entre) ou siga o que o README manda.
- SELinux: o Fedora vem com SELinux ativo. Se algo falhar sem motivo aparente em `systemctl --user` com binário em `/usr/local` ou `~/.local`, veja `journalctl -b | grep -i avc` e `sudo ausearch -m avc -ts recent`. Anote qualquer negação, é um achado que só o Fedora mostra.

Ao fim, desligue e restaure o snapshot se for repetir.

---

## Parte 4: NixOS limpo (testar os 3 métodos)

Os três métodos do README:

1. **`nix run`** (experimentar sem instalar)
2. **`nix profile add`** (instalar no perfil do usuário)
3. **overlay + `systemd.packages`** na configuração do sistema NixOS

### 4.1 Criar a VM (mais recursos: o veil será compilado dentro dela)

O pacote Nix do veil **compila a partir do código** (não há cache binário), roda `cargo test` no build e baixa muita coisa. Dê folga:

```bash
cd ~/vms
qemu-img create -f qcow2 disks/nixos.qcow2 50G
RAM=8G CPUS=6 ./run-vm.sh disks/nixos.qcow2 iso/<a-iso-grafica-do-nixos>.iso
```

### 4.2 Instalar pelo instalador gráfico (Calamares)

Escolha "Install NixOS" (o sistema ao vivo abre numa área de trabalho; um ícone **Install System**). No instalador:

| Item | O que escolher |
|---|---|
| Idioma, fuso, teclado | Português (Brasil) / São Paulo / ABNT2 |
| Usuário | crie o seu (ele recebe `wheel`, ou seja, sudo) |
| Desktop | **No desktop** (você entrará por um TTY, como nas outras distros) |
| **Allow unfree software** | **deixe DESMARCADO** (queremos testar o caso do usuário que não liberou nada) |
| Particionamento | "Erase disk", sem swap (ou swap pequeno), modo BIOS (GRUB) |

Instale. Ao terminar, desligue, e ligue **sem a ISO**:

```bash
RAM=8G CPUS=6 ./run-vm.sh disks/nixos.qcow2
```

Entre no TTY com seu usuário.

### 4.3 Habilitar flakes, sway, ferramentas e SSH

Esta é a **base mínima de um usuário que já tem NixOS** (flakes habilitados, um compositor). Edite a configuração do sistema:

```bash
sudo nano /etc/nixos/configuration.nix
```

Antes do `}` final, adicione:

```nix
  nix.settings.experimental-features = [ "nix-command" "flakes" ];
  programs.sway.enable = true;
  environment.systemPackages = with pkgs; [ foot git curl ];
  services.openssh.enable = true;
```

Salve (`Ctrl+O`, Enter, `Ctrl+X`) e aplique:

```bash
sudo nixos-rebuild switch
```

Se houver erro de sintaxe, a mensagem aponta a linha; corrija e rode de novo.

Configure o atalho do sway como Alt (opcional) e teclado:

```bash
mkdir -p ~/.config/sway
cp /etc/sway/config ~/.config/sway/config
sed -i 's/^set \$mod Mod4/set $mod Mod1/' ~/.config/sway/config
printf '\ninput * {\n    xkb_layout br\n}\n' >> ~/.config/sway/config
```

Teste que o sway abre (`sway`, `Alt+Enter`), saia (`Alt+Shift+E`) e desligue: `sudo systemctl poweroff`.

(Se a tela ficar preta: religa com `GL=0` e usa `WLR_RENDERER=pixman sway`.)

### 4.4 Snapshot da base

No host, com a VM desligada:

```bash
cd ~/vms
qemu-img snapshot -c base disks/nixos.qcow2
```

### 4.5 Preparar o tarball de mídia

Dentro da VM, no TTY ou em um terminal do sway:

```bash
cd ~ && curl -O http://10.0.2.2:8000/test.png -O http://10.0.2.2:8000/test-h264.mp4 -O http://10.0.2.2:8000/test-vp9.webm
```

(O servidor da Parte 0.8 precisa estar rodando.)

### 4.6 Qual versão do veil testar no Nix

Os três métodos usam `github:ordreth/veil`, que baixa do **GitHub**. Então só testam o seu código **depois de dar push**. Escolha uma:

- Teste antes do release: dê push em uma branch e use `github:ordreth/veil/NOME-DA-BRANCH` (ou o hash do commit) no lugar de `github:ordreth/veil` em todos os comandos abaixo.
- Teste do release: depois da tag (Parte 5), use `github:ordreth/veil/v1.0.0-beta.1`.
- O repositório precisa ser **público** (ou a VM, sem login, falha com 404; isso também é um achado se o README promete que funciona).

No resto desta parte, chamo isso de `$FLAKE`. Defina na VM:

```bash
FLAKE=github:ordreth/veil        # ou github:ordreth/veil/<branch ou tag>
```

### Método 1: `nix run` (sem instalar)

Entre no sway (`sway`) e abra um terminal. Em seguida:

```bash
nix run $FLAKE -- --version
```

Esperado: baixa dependências, **compila o veil** (vários minutos; o Rust do nixpkgs é compilado do zero), depois imprime a versão. Anote o tempo aproximado e se houve algum erro. Pontos de atenção:

- Se disser que o pacote é `unfree` e se recusa a construir: o README afirma que **o flake em si libera** (`allowUnfreePredicate` dentro do `flake.nix`), então isso seria um bug.
- Se reclamar do `nixpkgs` (versão antiga do Rust): anote a versão do NixOS (`nixos-version`).

Agora use de fato:

```bash
nix run $FLAKE -- daemon &
sleep 3
nix run $FLAKE -- status
nix run $FLAKE -- set --input ~/test.png
nix run $FLAKE -- set --input ~/test-h264.mp4
nix run $FLAKE -- set --input ~/test-vp9.webm
nix run $FLAKE -- quit
```

Anote: a primeira vez que o daemon abre ele cria um cache do GStreamer em `~/.cache/veil/gstreamer-registry.bin` (o README diz isso). Confirme: `ls -l ~/.cache/veil/`. Confirme que o wrapper define `MALLOC_ARENA_MAX`: com o daemon rodando, `cat /proc/$(pgrep -x veil | head -1)/environ | tr '\0' '\n' | grep MALLOC`. (Se o processo se chamar diferente, use `pgrep -a veil`.)

### Método 2: `nix profile add`

```bash
nix profile add $FLAKE
nix profile list
command -v veil
veil --version
ls ~/.nix-profile/lib/systemd/user 2>/dev/null
```

(Se o `nix` for antigo e não conhecer `add`, use `nix profile install` como o README diz.)

Teste manual igual ao do método 1, mas agora com `veil` direto:

```bash
veil daemon &
sleep 3
veil set --input ~/test.png
veil set --input ~/test-h264.mp4
veil status
veil quit
```

Anote:

- `man veil` funciona? As completions foram para algum lugar que o seu shell carrega? (O README só promete que o pacote as inclui.)
- A unit do systemd existe em `~/.nix-profile/lib/systemd/user/veil.service`, mas o systemd de usuário **não** a enxerga por aí. O README só descreve a unit no contexto da configuração do NixOS. Se um usuário que fez `nix profile add` quiser rodar como serviço, o README diz como? Anote a resposta (provavelmente: não diz).

Desfaça antes do método 3:

```bash
nix profile list
nix profile remove veil        # use o nome mostrado no list
```

### Método 3: overlay + `systemd.packages` na configuração do sistema

É o método do README. Vamos usá-lo numa **configuração flake** mínima, como o README pressupõe ("flake inputs: `veil.url = ...`").

**3a. Descobrir o canal instalado:**

```bash
nixos-version
```

Anote (por exemplo `26.05.xxxx`). O `nixpkgs` do flake abaixo deve ser do mesmo canal; troque `nixos-XX.XX` por ele. Se não souber, use `nixos-unstable` e anote que usou.

**3b. Criar o flake do sistema.** Como root:

```bash
sudo nano /etc/nixos/flake.nix
```

Conteúdo (troque `nixos-XX.XX`, e troque `github:ordreth/veil` pelo `$FLAKE` se estiver testando uma branch ou tag):

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-XX.XX";
    veil.url = "github:ordreth/veil";
  };

  outputs = { nixpkgs, veil, ... }: {
    nixosConfigurations.nixos = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        ./configuration.nix
        ({ pkgs, lib, ... }: {
          # Isto copia o bloco do README, sem o allowUnfreePredicate de propósito:
          nixpkgs.overlays = [ veil.overlays.default ];
          environment.systemPackages = [ pkgs.veil ];
          # The systemd user unit, started with the graphical session:
          systemd.packages = [ pkgs.veil ];
          systemd.user.services.veil.wantedBy = [ "graphical-session.target" ];
        })
      ];
    };
  };
}
```

O nome `nixos` em `nixosConfigurations.nixos` precisa ser o **hostname** da VM (o Calamares usa `nixos` por padrão; confirme com `hostname`).

**3c. Primeiro, o caso do usuário que NÃO liberou o unfree** (de propósito):

```bash
sudo nixos-rebuild switch --flake /etc/nixos#nixos
```

Esperado: **erro** dizendo que `veil` tem uma licença não livre (`unfree`) e como liberar. Anote o texto: ele é claro para um usuário? O README avisa sobre isso (sim, em "Because of the license, nixpkgs counts veil as unfree"). Se o erro vier com um aviso confuso, anote.

**3d. Agora libere, como o README manda.** Edite o flake e, no módulo, adicione a linha do README:

```nix
          nixpkgs.config.allowUnfreePredicate = pkg: builtins.elem (lib.getName pkg) [ "veil" ];
```

Aplique:

```bash
sudo nixos-rebuild switch --flake /etc/nixos#nixos
```

Esperado: compila o veil e conclui sem erro (vários minutos). Se você tiver marcado "Allow unfree" no instalador, esse passo 3c não falharia: por isso o deixamos desmarcado.

**3e. Verificar o resultado:**

```bash
command -v veil
ls -l /etc/systemd/user/ | grep -i veil
systemctl --user cat veil | head -8
```

Entre no sway (`sway`) e, no terminal dele:

```bash
systemctl --user status veil --no-pager
```

Aqui está o ponto crítico (igual ao S9 da Parte 1): a unit tem `wantedBy = graphical-session.target`. O sway do NixOS (`programs.sway.enable`) **não** necessariamente inicia esse target. Se o serviço estiver `inactive (dead)`:

- Anote isso, é um achado: o bloco do README prometeria "started with the graphical session" e não aconteceria com sway puro.
- Teste a solução provisória: `systemctl --user start veil` e veja se funciona (ambiente com `WAYLAND_DISPLAY`; se falhar com saída 5, o ambiente do systemd não tem `WAYLAND_DISPLAY`: teste `systemctl --user import-environment WAYLAND_DISPLAY SWAYSOCK` e inicie de novo).
- Isso vale também para Hyprland com ou sem UWSM; seu `dotfiles` provavelmente já resolve de algum jeito, mas um usuário novo não terá isso.

Teste os comandos:

```bash
veil status
veil set --input ~/test.png
veil set --input ~/test-h264.mp4
veil status
```

Teste a recuperação (kill -9 e restart) e a persistência (S8 e S9 da Parte 1, que se aplicam igualmente aqui).

Anote por fim: `man veil`, e as completions do bash (`veil <TAB>`).

### 4.7 Fechando o NixOS

Desligue a VM. Se quiser repetir o ciclo com outra variação (por exemplo, a mesma configuração mas com o `nixpkgs` do canal unstable), volte ao snapshot `base`:

```bash
qemu-img snapshot -a base disks/nixos.qcow2
```

Obs.: ao voltar ao snapshot você perde o cache da compilação (será preciso compilar o veil de novo). Se quiser economizar tempo nos ciclos, faça um segundo snapshot `com-flakes-sem-veil` antes do Método 1, e outro `depois-do-metodo-1` para guardar o `nix store` já preenchido.

---

## Parte 5: o caminho real do release (tag, GitHub, download)

Só faça isto **depois** de tudo acima passar com o tarball local, para não queimar uma tag com bugs de instalação já conhecidos. Cada passo `git` aqui é **sua** decisão: eu não commito nem dou push por você.

### 5.1 Corrigir o que apareceu nas Partes 2 a 4

Corrija o `install.sh`, o README, a unit etc., commite, e **repita as Partes 2 a 4** nas partes afetadas (use os snapshots `base`).

### 5.2 Validar a tag antes de criar (verificação local, não publica nada)

O workflow recusa tags cuja versão difere do `Cargo.toml` ou sem seção no `CHANGELOG.md`. Teste localmente:

```bash
cd ~/projects/ordreth/veil
scripts/release/notes.sh v1.0.0-beta.1 | head -20
```

Esperado: imprime as notas da versão. Se der erro, é o mesmo erro que o GitHub daria.

### 5.3 Criar e enviar a tag

```bash
git tag -a v1.0.0-beta.1 -m "veil 1.0.0-beta.1"
git push origin main
git push origin v1.0.0-beta.1
```

(Empurre a branch antes da tag. Uma tag publicada deve ser considerada imutável: se o workflow falhar por problema do **workflow**, é melhor corrigir e usar `v1.0.0-beta.2` do que reescrever a tag.)

### 5.4 Acompanhar o workflow e conferir o release

No navegador: aba **Actions → Release**. Os jobs são `notes`, `build (x86_64)`, `build (aarch64)` e `publish`. Depois, em **Releases**:

- Marcado como **Pre-release** (a versão tem `-beta.1`).
- Assets: os dois tarballs (`x86_64` e `aarch64`) e o `SHA256SUMS`.
- As notas são a seção do `CHANGELOG.md`.

Teste num navegador **sem login** (janela anônima) que a página do release e os downloads abrem. Se o repositório for privado, os `curl` do README falham: é a primeira coisa a descobrir.

### 5.5 Repetir S2 a S11 com a URL real

Numa VM limpa (volte ao snapshot `base` do Arch ou Fedora), troque só a variável `base` do início da Parte 1:

```bash
version=1.0.0-beta.1
tarball=veil-$version-$(uname -m)-unknown-linux-gnu.tar.gz
base=https://github.com/ordreth/veil/releases/download/v$version
```

Observação: o README usa `version=1.0.0` no exemplo; uma pessoa vai ter que trocar. Isso é aceitável, mas anote se confunde (e se você quer um exemplo que sirva para o beta).

Execute **S2 a S10** (pode pular S7 se já viu).

### 5.6 A verificação de proveniência (attestation)

O README manda `gh attestation verify "$tarball" --repo ordreth/veil`. Em uma VM:

```bash
sudo pacman -S --needed github-cli        # Arch
sudo dnf install -y gh                    # Fedora
cd ~/dl-veil
gh attestation verify "$tarball" --repo ordreth/veil
```

Anote:

- Se pede `gh auth login` (um usuário comum não terá conta): é um achado se o README apresenta isso como passo normal. Se pedir, o README deveria dizer que é opcional e que exige login.
- Se a verificação passa com a mensagem `Verification succeeded!`.

### 5.7 O caminho do Nix com a tag real

Na VM NixOS (volte ao snapshot `base`), refaça os 3 métodos da Parte 4 com:

```bash
FLAKE=github:ordreth/veil/v1.0.0-beta.1
```

(O Método 3 usa a URL no flake: `veil.url = "github:ordreth/veil/v1.0.0-beta.1";`.) Pelo menos o Método 1 e o 3.

---

## Parte 6: o que levar de volta para mim se algo falhar

Se um passo falhar ou algo parecer estranho, junte isto **antes** de reverter o snapshot:

Dentro da VM:

```bash
cat /etc/os-release | head -3
uname -r
gst-inspect-1.0 --version | head -1
veil --version
systemctl --user status veil --no-pager
journalctl --user -u veil -b --no-pager | tail -60
swaymsg -t get_outputs | head -20
```

Para trazer o texto para o host (precisa do sshd habilitado):

```bash
# no host
ssh -p 2222 SEU_USUARIO@127.0.0.1 'journalctl --user -u veil -b --no-pager' > falha-arch-veil.log
```

Para cada achado, anote em uma linha: **distro e versão · passo (Sx ou método) · o que o README diz · o que aconteceu · o que resolveu**. Com isso eu consigo corrigir o `install.sh`, a unit e o README rapidamente.

### Tabela final para preencher

| Verificação | Arch | Fedora | NixOS (1: run) | NixOS (2: profile) | NixOS (3: overlay) |
|---|---|---|---|---|---|
| README foi seguido sem passos extras | | | | | |
| Dependências instaladas pelo comando do README | | | n/a | n/a | n/a |
| `install.sh --prefix ~/.local` | | | n/a | n/a | n/a |
| `veil` no PATH sem ajuste | | | | | |
| `man veil` / completions | | | | | |
| Imagem aparece | | | | | |
| Vídeo H.264 toca | | | | | |
| Unit do systemd encontrada | | | n/a | n/a | |
| Daemon sobe pela unit | | | n/a | n/a | |
| Sobe sozinho no login | | | n/a | n/a | |
| Reinicia depois de `kill -9` | | | n/a | n/a | |
| Wallpaper volta depois de reiniciar | | | | | |
| `sudo ./install.sh` (`/usr/local`) | | | n/a | n/a | n/a |
| `--uninstall` limpa tudo | | | n/a | n/a | n/a |
| `gh attestation verify` | | | n/a | n/a | n/a |

Hipóteses de achados prováveis (para você saber o que procurar, não conclusões):

1. A unit não inicia sozinha no login em sway puro (sem `graphical-session.target`).
2. `WAYLAND_DISPLAY` / `SWAYSOCK` não chegam ao systemd do usuário.
3. `~/.local/bin` fora do `PATH` (Arch).
4. `gh attestation verify` pedindo login.
5. O exemplo de download do README usa `version=1.0.0`, não o beta.
6. O Método 3 do Nix pressupõe uma configuração com flakes; quem usa `configuration.nix` clássico (sem flake) não tem instrução no README.
7. `nix profile add` instala o pacote mas não deixa a unit utilizável.
