---
title: "Roteiro"
description: "O que falta antes da 1.0 e o que vem depois dela."
group: "Reference"
order: 10
source: "docs/roadmap.md"
source_hash: "575211a7255ab43face66fd50d731673baf75a4f902525db78477e26c2f9232a"
---

O que falta antes do veil 1.0 e o que fica deliberadamente para depois dela. Tudo o que está na tabela de recursos do [README](/apps/veil) está implementado e testado; o que resta antes do lançamento é transformar as últimas verificações feitas à mão em testes, e publicar. O veil é verificado apenas por testes automatizados ([`testing.md`](https://github.com/ordreth/veil/blob/stable/docs/testing.md)).

**O que é a 1.0:** um daemon de papel de parede que o usuário altera manualmente (`veil set` e os demais comandos, um arquivo de configuração), que sobrevive a reinicializações e se recupera sozinho de uma falha, cujas afirmações sobre uso de recursos são medidas ([`measurements.md`](/apps/veil/docs/measurements)), e que é empacotado para o Nix e como binários de lançamento. O vídeo é reproduzido sem áudio.

## O beta (1.0.0-beta.1)

O primeiro lançamento público, 1.0.0-beta.1 (publicado em 2026-11-04), é um beta, e ele flexibiliza o checklist da 1.0 abaixo. **Obrigatório, e feito:** o workflow de lançamento executado na própria tag do lançamento. **Não obrigatório, e declarado como não testado na [matriz de suporte](/apps/veil/docs/troubleshooting#matriz-de-suporte):** o teste de longa duração de 24 horas, uma execução em ARM, gráficos Intel e AMD, outras distribuições e outros compositores. O beta é gratuito e de código-fonte disponível ([`LICENSE`](/apps/veil/terms)), não aceita issues nem pull requests (os relatos vão para o GitHub Discussions) e é distribuído como os tarballs de lançamento, o flake do Nix e, após o lançamento, um pacote `veil-bin` do AUR.

## Antes da 1.0

### Testes que faltam escrever

Nenhum: desde 2026-11-20, toda verificação feita à mão quando o seu recurso foi adicionado é um teste ou um script ([`testing.md`](https://github.com/ordreth/veil/blob/stable/docs/testing.md)). Três deles ainda não tinham rodado no CI quando foram escritos, porque precisam do Nix ou de contêineres: as verificações do flake `video` e `completions`, e o workflow `Installation on distributions`; a primeira execução verde deles conclui isto. O que nenhum teste alcança (a renderização do próprio Hyprland, placas de vídeo e arquiteturas que o CI não tem, o systemd agindo sobre o que o daemon lhe diz) está listado em [`testing.md`](https://github.com/ordreth/veil/blob/stable/docs/testing.md), "Not covered by any test", e não é condição para o lançamento.

### O workflow de lançamento

O `.github/workflows/release.yml` rodou no GitHub para a tag `v1.0.0-beta.1` (2026-11-04) e publicou o lançamento como pré-lançamento com os tarballs x86_64 e aarch64 e o `SHA256SUMS`. Antes da 1.0:

- Confirmar ou mudar a decisão do problema conhecido 8 (GIFs animados e perda de quadros com o GStreamer do Debian 12 e do Ubuntu 22.04 e 24.04: documentado, sem solução alternativa).

### Checklist de lançamento

Tudo isto deve ser verdade para a 1.0:

- CI verde na `main`: `cargo fmt --check`, clippy com `-D warnings`, `cargo test`, `nix build`, `cargo deny check`, a compilação com a MSRV e a compilação de lançamento.
- Toda verificação antes feita à mão é um teste (feito em 2026-11-20, [`testing.md`](https://github.com/ordreth/veil/blob/stable/docs/testing.md)), verde no CI, incluindo as verificações do flake e o workflow `Installation on distributions`. De uma falha que se recupera sozinha, a parte do daemon é testada (ele volta com os papéis de parede salvos depois de um `kill -9`, e informa ao systemd aquilo de que a unidade depende); o reinício pelo próprio systemd não pode rodar onde os testes rodam.
- Teste de longa duração: memória residente, threads e descritores estáveis ao longo de 24 horas de um vídeo em repetição e de 500 trocas de papel de parede (`scripts/bench/soak.sh loop --duration 86400`, uma vez com o decodificador por hardware e uma vez com `GST_PLUGIN_FEATURE_RANK=nvh264dec:0`, que cresceu aos poucos nos primeiros minutos; `scripts/bench/soak.sh switch`; ambos com o `MALLOC_ARENA_MAX=1` distribuído).
- Ocioso: 0.0 % de CPU para imagens estáticas e para vídeo totalmente coberto ou pausado, como os testes headless o medem.
- Todo exemplo dos documentos é executado por um teste, como os de [`ipc.md`](/apps/veil/docs/ipc) já são.
- Nenhum `unwrap()` em caminhos de produção e todo `unsafe` documentado (os lints em `Cargo.toml` impõem isso; confirmar que nada os silenciou).
- Uma revisão de segurança do socket de controle ([`SECURITY.md`](/apps/veil/security)).
- O `CODE_OF_CONDUCT.md` informa contact@ordreth.com como contato, o relato privado de vulnerabilidades está ativado no repositório e o Discussions tem sua categoria "Reports" (configurações no GitHub).
- O `CHANGELOG.md` e a versão em `Cargo.toml` e `flake.nix` foram atualizados.

### Decisões em aberto

- **Problema conhecido 1:** corrigido no veil pelo seu sink de vídeo próprio (2026-11-09). Se a falha do `waylandsink` deve ser relatada ao projeto original, afinal, fica em aberto; ela não afeta mais o veil.
- **Outras distribuições e compositores:** o veil é desenvolvido em NixOS e Hyprland, no nosso próprio ritmo. Outro sistema é olhado quando as pessoas que o usam demonstram interesse, não com antecedência ([Discussions, "Reports"](https://github.com/ordreth/veil/discussions/categories/reports)).
- **O nome do crate:** `veil` já está em uso no crates.io; decidiu-se em 2026-10-19 não publicar lá para a 1.0.

## Depois da 1.0

Nada disso começa antes do lançamento. Nenhum destes itens é necessário para trocar papéis de parede manualmente, e esperar não prende nada: o arquivo de estado tem uma versão, o arquivo de configuração não tem chaves para recursos que não existem, e os novos comandos de IPC são aditivos.

- **Opções de reprodução:** áudio (silenciado hoje; com um pipeline por saída, ele primeiro precisa de uma decisão sobre qual saída toca o som), um limite de taxa de quadros (o `videorate` descarta quadros depois de decodificados, então se isso economiza algo é uma pergunta para as medições), velocidade de reprodução, pontos de repetição, uma preferência de decodificador. Forçar a decodificação por software já funciona por meio do `GST_PLUGIN_FEATURE_RANK` ([`troubleshooting.md`](/apps/veil/docs/troubleshooting)).
- **Playlists e rotação:** `veil set <dir> --interval 30m [--shuffle]`, `next`/`prev`, em um timer do `calloop` e não em uma thread. A rotação pode ser feita por script com `veil set` enquanto isso.
- **Transições:** um fade opcional entre imagens. A passagem entre os dois engines do `RenderState` é a base sobre a qual um fade seria construído.
- **Mais backends de compositor e gatilhos:** os backends do niri e do river (sway e Hyprland existem), e pausar quando a sessão está ociosa ou na bateria.
- **Depois de substituir o `waylandsink`** (feito em 2026-11-09, problema conhecido 1): quadros em dmabufs por meio do `zwp_linux_dmabuf_v1` (VA-API em Intel e AMD, sem conversão na CPU: feito em 2026-11-20), uma conversão na GPU para os decodificadores da NVIDIA (problema conhecido 7), e um pipeline que passa para a nova superfície da sua saída em vez de recomeçar (problema conhecido 2).
- **Um loop de frame callback na taxa de quadros do vídeo** em vez da taxa da tela (problema conhecido 6): não tentado.
- **Empacotamento:** módulos de NixOS e home-manager (`programs.veil.enable`, `settings` renderizado para TOML), AUR, pacotes `.deb` e `.rpm`. O Flatpak está fora do escopo: por padrão ele não alcança o socket de IPC do compositor.
- **Higiene de dependências:** menos recursos do `notify-rust` ou uma chamada direta a `org.freedesktop.Notifications`; atualizações do `gstreamer` (0.22), do `thiserror` (1 para 2) e do `smithay-client-toolkit`, que removem as exceções em `deny.toml`; os recursos do `image` que o veil não usa (os recursos padrão compilam um codificador AV1 e o codec OpenEXR); Dependabot ou Renovate.
- **Testes e documentação:** fuzzing (`cargo-fuzz` para o parser de requisições de IPC, o parser da configuração, o parser de eventos do Hyprland e `engine::media::sniff`; `proptest` para a geometria de escala), uma comparação com `mpvpaper` e `swww` nos mesmos clipes e hardware (`docs/benchmarks.md`), rustdoc para as traits públicas e `#![warn(missing_docs)]`.
- **Diagnóstico:** `--log-level`, `--log-format {pretty,json}` e `veil doctor` (globals do Wayland, plugins do GStreamer, VA-API, permissões do socket, configuração).
- **Depois do lançamento:** um esquema de labels para a triagem de bugs, uma cadência de lançamentos (lançamentos de correção para as correções), uma política de depreciação para o alias `--daemon` e os relatos de compatibilidade dos usuários.
