# veil: tracking do teste em VM (NixOS)

Arquivo local, NÃO versionado (não faça `git add`). Roteiro geral: `veil-install-test-plan.md`.
Para retomar: leia "Estado" e continue no primeiro item `[ ]` de "Checklist".

## Estado (atualizado em 2026-11-04)
- Método 1 (`nix run github:ordreth/veil`): OK. `set` com imagem e vídeo funciona em 1920x1080.
- Achado 2 confirmado como bug do veil (ver "Achados").
- T1 concluído. T2 e T3 só parciais. T4 a T7 dispensados por decisão do usuário (2026-11-04): a VM já mostrou que um usuário instala e usa o veil num NixOS novo. Achado 2 consertado em `src/wayland/target.rs` (`exact_canvas` + 3 testes), ainda NÃO commitado nem testado na VM. Próximos, um de cada vez: (a) limpar a árvore para um commit limpo, (b) atualizar a documentação, (c) atualizar o site do veil (outro repo).

## Checklist
- [x] M1: `nix run` compila, `--version`, daemon, `set` imagem e vídeo
- [x] T1: Método 2, `nix profile add` (OK)
- [x] T2: `kill -9` no daemon (OK: o novo daemon removeu o socket velho e subiu limpo)
- [~] T3: persistência do wallpaper (parcial, achado 11)
- [-] T4: Método 3 sem `allowUnfreePredicate` (dispensado)
- [-] T5: Método 3 com `allowUnfreePredicate` (dispensado)
- [-] T6: serviço systemd no login (dispensado)
- [-] T7: README sobre flakes etc. (dispensado)
- [ ] Depois: Arch e Fedora (Partes 2 e 3 do plano), Parte 5 (tag e release)

## Ambiente (rápido)
- Host: NixOS, Hyprland, teclado US 60%. A VM é um flake em `~/vms/nixos-clean/flake.nix` (fora do repo).
- Subir a VM (terminal do HOST):
  ```
  cd ~/vms/nixos-clean
  nix run .#nixosConfigurations.vm.config.system.build.vm
  ```
  Disco: `nixos.qcow2` na mesma pasta (apagar = VM limpa, perde tudo, inclusive o store compilado).
- Usuário `teste`, senha `teste`, autologin no TTY.
- Entrar por SSH (terminal do HOST, onde colar funciona):
  ```
  ssh -p 2222 -o UserKnownHostsFile=/dev/null -o StrictHostKeyChecking=no teste@127.0.0.1
  ```
- O sway precisa estar aberto: na JANELA da VM, no TTY, digite `sway`. Atalho do sway é Alt (Alt+Enter abre terminal).
- Variáveis para usar o sway pelo SSH. Já gravadas no `~/.bashrc` da VM? Se `echo $WAYLAND_DISPLAY` vier vazio, cole:
  ```
  cat >> ~/.bashrc <<'EOF'
  export XDG_RUNTIME_DIR=/run/user/$(id -u) WAYLAND_DISPLAY=wayland-1
  export SWAYSOCK=$(ls /run/user/$(id -u)/sway-ipc.*.sock 2>/dev/null | head -1)
  EOF
  ```
  e depois reconecte o SSH (com o sway já aberto).
- Resolução da saída: use SEMPRE 1920x1080 (ver achado 2):
  ```
  swaymsg output Virtual-1 mode 1920x1080
  swaymsg -t get_outputs | grep -E '"width"|"height"' | head -2
  ```
  O primeiro bloco deve mostrar 1920 e 1080. Vale refazer se a janela da VM mudar de tamanho.
- Disco novo: refazer config do sway (Mod1 + `xkb_layout us`), `~/foto.jpg` e `~/video.mp4`:
  ```
  mkdir -p ~/.config/sway && cp /etc/sway/config ~/.config/sway/config
  sed -i 's/^set \$mod.*/set $mod Mod1/' ~/.config/sway/config
  printf 'input * xkb_layout us\n' >> ~/.config/sway/config
  curl -L -o ~/foto.jpg https://picsum.photos/1920/1080
  nix shell nixpkgs#ffmpeg -c ffmpeg -f lavfi -i testsrc=duration=5:size=1280x720:rate=30 -pix_fmt yuv420p ~/video.mp4
  ```
- Dois terminais SSH ao mesmo tempo: **A** roda o daemon em primeiro plano (mostra o log), **B** manda comandos. Os dois precisam das variáveis acima (senão o `set` procura `veil-wayland-0.sock` e diz que não há daemon).

## Quando limpar a VM
Não precisa limpar entre T1, T2 e T3: só garanta que não há daemon velho (`pkill -x veil`). NÃO apague `nixos.qcow2` entre etapas: perde o store já compilado (recompilar leva vários minutos e já estourou a RAM uma vez). Para simular usuário novo sem apagar o disco, confira primeiro com `ls ~/.cache/veil ~/.local/state/veil` e só então remova o que for do veil.

## T1: Método 2, `nix profile add`
Objetivo: instalar no perfil do usuário e usar o comando `veil` direto, sem `nix run`.

1. SSH **A**. Garanta que não há daemon velho: `pkill -x veil; sleep 1; pgrep -a veil || echo limpo`
2. Instalar (compila de novo se o store estiver limpo, pode levar vários minutos; se o store já tem o veil do Método 1, é rápido):
   ```
   nix profile add github:ordreth/veil
   nix profile list
   command -v veil
   veil --version
   ```
   Anote: houve erro? (se `add` não existir, use `nix profile install`.) Qual o nome que `nix profile list` mostra?
3. Ver o que o pacote instalou:
   ```
   ls ~/.nix-profile/lib/systemd/user 2>/dev/null
   man veil | head -5
   ```
4. Usar. Terminal **A**: `veil daemon`. Terminal **B** (com as variáveis):
   ```
   veil status
   veil set --input ~/foto.jpg
   veil set --input ~/video.mp4
   veil status
   veil quit
   ```
   Esperado: wallpaper muda na janela da VM, `quit` encerra o daemon no terminal A.
5. Pergunta para anotar: o README diz como rodar como serviço para quem usou `nix profile add`? (A unit fica em `~/.nix-profile/lib/systemd/user/veil.service`, mas o systemd de usuário não olha ali.)
6. Desfazer antes do Método 3:
   ```
   nix profile remove veil
   command -v veil || echo removido
   ```
   (use o nome mostrado em `nix profile list` se não for `veil`).

Resultado: _(preencher)_

## T2: `kill -9` no daemon
Objetivo: ver se o veil se recupera de uma morte brusca (socket velho, wallpaper preso).

1. Terminal A: `veil daemon` (ou `nix run github:ordreth/veil -- daemon` se o T1 não deixou `veil` instalado).
2. Terminal B: `veil set --input ~/foto.jpg`, confirme que apareceu.
3. Matar: `pkill -9 -x veil; sleep 1; pgrep -a veil || echo morto`
4. Ver o socket que sobrou: `ls -l /run/user/1000/veil-wayland-1.sock`
5. Tentar falar com ele: `veil status` (esperado: erro claro dizendo que não há daemon, ou que o socket é velho).
6. Subir de novo no terminal A: `veil daemon`. Esperado: sobe sem reclamar do socket velho. O wallpaper some ou fica congelado enquanto o daemon está morto? Anote.

Resultado: _(preencher)_

## T3: persistência do wallpaper
Objetivo: o wallpaper volta quando o daemon reinicia (o log diz `showing the saved wallpapers`).

1. Daemon no terminal A, `veil set --input ~/foto.jpg` no B.
2. `veil quit` no B. Suba o daemon de novo no A, **sem** `--input`.
3. Esperado: a foto volta sozinha. Anote o log do daemon.
4. Repita com o vídeo (`~/video.mp4`).
5. Reiniciar a sessão: na janela da VM, `Alt+Shift+E` (sai do sway), `sway` de novo, `veil daemon`. O wallpaper salvo volta?

Resultado: _(preencher)_

## T4 a T6: Método 3 (overlay + `systemd.packages`)
Atenção: nossa VM é um `vmVariant` de flake no host. Não há `/etc/nixos` na VM para rodar `nixos-rebuild`, então a configuração é editada **no host**, em `~/vms/nixos-clean/flake.nix`, e a VM é reconstruída ao subir. (Eu não leio `~/vms`, então quando chegarmos aqui cole o `flake.nix` no chat ou me diga que posso ler, e eu indico as linhas exatas.)

Bloco do README, que será acrescentado ao módulo da VM (sem `allowUnfreePredicate` no T4):
```nix
({ pkgs, lib, ... }: {
  nixpkgs.overlays = [ veil.overlays.default ];
  environment.systemPackages = [ pkgs.veil ];
  systemd.packages = [ pkgs.veil ];
  systemd.user.services.veil.wantedBy = [ "graphical-session.target" ];
})
```
O input `veil.url = "github:ordreth/veil";` precisa existir nos `inputs` do flake.

- **T4**: subir a VM com o bloco acima SEM `allowUnfreePredicate`. Esperado: o build no host falha dizendo que `veil` é unfree e como liberar. Anote o texto do erro e se um usuário novo entenderia.
- **T5**: acrescentar `nixpkgs.config.allowUnfreePredicate = pkg: builtins.elem (lib.getName pkg) [ "veil" ];` e subir de novo. Esperado: compila e a VM sobe com `veil` no PATH.
- **T6**: na VM, depois de entrar no sway:
  ```
  command -v veil
  ls -l /etc/systemd/user/ | grep -i veil
  systemctl --user status veil --no-pager
  ```
  Esperado pelo README: serviço ativo com a sessão gráfica. Com sway puro, o `graphical-session.target` normalmente NÃO sobe, e o serviço fica `inactive (dead)`: isso é achado. Teste o paliativo: `systemctl --user start veil`; se falhar por falta de `WAYLAND_DISPLAY`, `systemctl --user import-environment WAYLAND_DISPLAY SWAYSOCK` e iniciar de novo.

Resultado: _(preencher)_

## T7: README
Marcar o que o README não diz e foi preciso fazer: flakes habilitados (ainda não confirmado, a VM já os tinha ligados), `allowUnfree`, variáveis do Wayland, serviço para quem usa `nix profile`.

## Achados
1. README não diz que os flakes precisam estar habilitados (não confirmado: a VM já os tinha).
2. **Bug real do veil, confirmado.** `veil set`/`daemon` falha com `Failed to create SHM buffer: a canvas of N bytes does not hold WxH pixels` quando `largura x altura x 4` não é múltiplo de 64. Exemplos medidos: 639x1009 (canvas 2579008 contra 2579004 esperados) e 1900x1009 (7668416 contra 7668400). O `SlotPool` devolve o canvas arredondado para cima a múltiplo de 64 bytes e `compose` (`src/engine/image.rs`) exige o tamanho exato. NÃO é corrida de redimensionamento: ocorreu num daemon recém-iniciado. Tamanhos comuns (1920x1080, 1280x720, 1366x768) são múltiplos de 64 e não são afetados. Conserto sugerido: passar ao `compose` só os primeiros `stride x altura` bytes (em `src/wayland/target.rs`, `present`) ou relaxar a checagem para `>=`. **Decisão do usuário: consertar antes da tag ou registrar como problema conhecido da beta. Código não foi alterado.**
3. Queda do daemon com o vídeo: provavelmente a mesma causa do 2, não reproduzida em 1920x1080. Tratar como resolvido, sem prova.
4. Sem serviço de notificações na VM: `Failed to dispatch desktop notification: ServiceUnknown` (warning, esperado em sway puro).
5. O virtio-vga só oferece modos fixos (5120x2160 ... 640x480) mais o tamanho atual da janela. 1280x720 NÃO existe; `output Virtual-1 resolution 1280x720` é ignorado em silêncio. Use `mode 1920x1080`.
6. `swaymsg` sem erro imprime nada.
7. O `veil set` procura `veil-<WAYLAND_DISPLAY>.sock`; sem `WAYLAND_DISPLAY` no shell ele procura `wayland-0` e diz "no veil daemon is running".
8. Hyprland do host usa o parser novo: `hyprctl keyword` não funciona (usar `dispatch` ou `eval`).
9. T1 OK: `nix profile add` instala `veil` no PATH; `--version`, `man veil`, `status`, `set` (imagem e vídeo, 30 fps, 0 frames perdidos, decoder avdec_h264 por software) e `quit` funcionam. A unit existe em `~/.nix-profile/lib/systemd/user/veil.service`.
10. T2 inconclusivo: com `nix run`, `pkill -9 -x veil` não matou nada (o daemon seguiu com o mesmo pid e o `status` respondeu), porque o nome do processo não é exatamente `veil`. O daemon foi encerrado depois no terminal dele, o socket sumiu e um novo `daemon` subiu limpo. O caso de socket velho depois de um SIGKILL de verdade NÃO foi testado.
11. T3 parcial: o `status` mostrava `default: foto.jpg` e o daemon avisa `showing the saved wallpapers`, mas não vimos o wallpaper voltar depois de reiniciar.
12. NÃO testados (dispensados): Método 3 (overlay + `systemd.packages`, comportamento do unfree, `graphical-session.target` com sway puro), serviço no login, README sobre flakes.
