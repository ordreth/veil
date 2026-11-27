---
title: "Roteiro"
description: "O que falta antes da 1.0 e o que vem depois dela."
group: "Reference"
order: 10
source: "docs/roadmap.md"
source_hash: "20a32433abc203e85e259aeb312dbae28a03a2241669fa6cb5981af083c72e0c"
---

O que falta antes do veil 1.0 e o que fica deliberadamente para depois dela. Tudo o que está na tabela de recursos do [README](/apps/veil) está implementado e testado; o que resta antes do lançamento é verificar à mão onde nenhum teste alcança, e publicar.

**O que é a 1.0:** um daemon de papel de parede que o usuário altera manualmente (`veil set` e os demais comandos, um arquivo de configuração), que sobrevive a reinicializações e se recupera sozinho da falha do GStreamer do [problema conhecido 1](/apps/veil/docs/known-issues), cujas afirmações sobre uso de recursos são medidas ([`measurements.md`](/apps/veil/docs/measurements)), e que é empacotado para o Nix e como binários de lançamento. O vídeo é reproduzido sem áudio.

## O beta (1.0.0-beta.1)

O primeiro lançamento público, 1.0.0-beta.1 (publicado em 2026-11-04), é um beta, e ele flexibiliza o checklist da 1.0 abaixo. **Obrigatório, e feito:** o workflow de lançamento executado na própria tag do lançamento. **Não obrigatório, e declarado como não testado na [matriz de suporte](/apps/veil/docs/troubleshooting#matriz-de-suporte):** o checklist completo de [`manual-tests.md`](https://github.com/ordreth/veil/blob/main/docs/manual-tests.md), "In a real Hyprland session" (só parte dele foi executada, veja abaixo), o teste de longa duração de 24 horas, uma execução em ARM, gráficos Intel e AMD, outras distribuições e outros compositores. O beta é gratuito e de código-fonte disponível ([`LICENSE`](/apps/veil/docs/license)), não aceita issues nem pull requests (os relatos vão para o GitHub Discussions) e é distribuído como os tarballs de lançamento, o flake do Nix e, após o lançamento, um pacote `veil-bin` do AUR.

## Antes da 1.0

### Verificações em uma sessão real do Hyprland

Quase tudo foi verificado em um sway headless; no Hyprland 0.56.2 foram executados apenas imagens, um vídeo em dois monitores e a pausa atrás de uma janela em tela cheia (2026-10-20). O checklist está em [`manual-tests.md`](https://github.com/ordreth/veil/blob/main/docs/manual-tests.md), "In a real Hyprland session": a pausa atrás de janelas em tela cheia, a tela de bloqueio e o DPMS, se um vídeo em `fill` fica parado a cada repetição (problema conhecido 5), se ele é reproduzido com fluidez (problema conhecido 6), a unidade do systemd, a recuperação de falhas, o logout, o autocompletar e a linha do Hyprland da tabela de compatibilidade em [`troubleshooting.md`](/apps/veil/docs/troubleshooting#compatibilidade). Um problema encontrado ali é corrigido, ou registrado como problema conhecido, antes do lançamento.

### O workflow de lançamento

O `.github/workflows/release.yml` rodou no GitHub para a tag `v1.0.0-beta.1` (2026-11-04) e publicou o lançamento como pré-lançamento com os tarballs x86_64 e aarch64 e o `SHA256SUMS` (veja [`manual-tests.md`](https://github.com/ordreth/veil/blob/main/docs/manual-tests.md), "The release tarball", para o que foi verificado localmente antes). Antes da 1.0:

- Executar o tarball aarch64 em uma máquina ARM com um compositor: nada exibiu uma imagem ou um vídeo com ele até agora.
- Confirmar ou mudar a decisão do problema conhecido 8 (GIFs animados e perda de quadros com o GStreamer do Debian 12 e do Ubuntu 22.04 e 24.04: documentado, sem solução alternativa).

### Checklist de lançamento

Tudo isto deve ser verdade para a 1.0:

- CI verde na `main`: `cargo fmt --check`, clippy com `-D warnings`, `cargo test`, `nix build`, `cargo deny check`, a compilação com a MSRV e a compilação de lançamento.
- Uma falha se recupera sozinha: sob a unidade do systemd, o daemon volta com o papel de parede salvo (problema conhecido 1, opção D).
- Teste de longa duração: memória residente, threads e descritores estáveis ao longo de 24 horas de um vídeo em repetição e de 500 trocas de papel de parede (`scripts/bench/soak.sh loop --duration 86400`, uma vez com o decodificador por hardware e uma vez com `GST_PLUGIN_FEATURE_RANK=nvh264dec:0`, que cresceu aos poucos nos primeiros minutos; `scripts/bench/soak.sh switch`; ambos com o `MALLOC_ARENA_MAX=1` distribuído).
- Ocioso: 0.0 % de CPU para imagens estáticas e para vídeo totalmente coberto ou pausado.
- Verificado em pelo menos o Hyprland e outro compositor wlroots, em gráficos Intel, AMD e NVIDIA (ou os não suportados documentados).
- Todo exemplo dos documentos roda como escrito.
- Nenhum `unwrap()` em caminhos de produção e todo `unsafe` documentado (os lints em `Cargo.toml` impõem isso; confirmar que nada os silenciou).
- Uma revisão de segurança do socket de controle ([`SECURITY.md`](/apps/veil/docs/security)).
- O `CODE_OF_CONDUCT.md` informa a conta do GitHub do mantenedor como contato, o relato privado de vulnerabilidades está ativado no repositório e o Discussions tem sua categoria "Reports" (veja [`manual-tests.md`](https://github.com/ordreth/veil/blob/main/docs/manual-tests.md), "Governance files").
- O `CHANGELOG.md` e a versão em `Cargo.toml` e `flake.nix` foram atualizados.

### Decisões em aberto

- **Problema conhecido 1:** se a falha do `waylandsink` deve ser relatada ao projeto original, afinal. Não bloqueia.
- **Outras distribuições e compositores:** o veil é desenvolvido em NixOS e Hyprland, no ritmo do mantenedor. Outro sistema é olhado quando as pessoas que o usam demonstram interesse, não com antecedência ([Discussions, "Reports"](https://github.com/ordreth/veil/discussions/categories/reports)).
- **O nome do crate:** `veil` já está em uso no crates.io; decidiu-se em 2026-10-19 não publicar lá para a 1.0.

## Depois da 1.0

Nada disso começa antes do lançamento. Nenhum destes itens é necessário para trocar papéis de parede manualmente, e esperar não prende nada: o arquivo de estado tem uma versão, o arquivo de configuração não tem chaves para recursos que não existem, e os novos comandos de IPC são aditivos.

- **Opções de reprodução:** áudio (silenciado hoje; com um pipeline por saída, ele primeiro precisa de uma decisão sobre qual saída toca o som), um limite de taxa de quadros (o `videorate` descarta quadros depois de decodificados, então se isso economiza algo é uma pergunta para as medições), velocidade de reprodução, pontos de repetição, uma preferência de decodificador. Forçar a decodificação por software já funciona por meio do `GST_PLUGIN_FEATURE_RANK` ([`troubleshooting.md`](/apps/veil/docs/troubleshooting)).
- **Playlists e rotação:** `veil set <dir> --interval 30m [--shuffle]`, `next`/`prev`, em um timer do `calloop` e não em uma thread. A rotação pode ser feita por script com `veil set` enquanto isso.
- **Transições:** um fade opcional entre imagens. A passagem entre os dois engines do `RenderState` é a base sobre a qual um fade seria construído, e teria de ser refeita se o `waylandsink` for substituído.
- **Mais backends de compositor e gatilhos:** os backends do niri e do river (sway e Hyprland existem), e pausar quando a sessão está ociosa ou na bateria.
- **Substituir o `waylandsink`:** um sink próprio do veil eliminaria os problemas conhecidos 1 e 2, o custo de conversão do problema conhecido 7 na NVIDIA e os despertares medidos em [`measurements.md`](/apps/veil/docs/measurements). Grande.
- **Um loop de frame callback na taxa de quadros do vídeo** em vez da taxa da tela (problema conhecido 6): não tentado.
- **Empacotamento:** módulos de NixOS e home-manager (`programs.veil.enable`, `settings` renderizado para TOML), AUR, pacotes `.deb` e `.rpm`. O Flatpak está fora do escopo: por padrão ele não alcança o socket de IPC do compositor.
- **Higiene de dependências:** menos recursos do `notify-rust` ou uma chamada direta a `org.freedesktop.Notifications`; atualizações do `gstreamer` (0.22), do `thiserror` (1 para 2) e do `smithay-client-toolkit`, que removem as exceções em `deny.toml`; os recursos do `image` que o veil não usa (os recursos padrão compilam um codificador AV1 e o codec OpenEXR); Dependabot ou Renovate.
- **Testes e documentação:** fuzzing (`cargo-fuzz` para o parser de requisições de IPC, o parser da configuração, o parser de eventos do Hyprland e `engine::media::sniff`; `proptest` para a geometria de escala), uma comparação com `mpvpaper` e `swww` nos mesmos clipes e hardware (`docs/benchmarks.md`), rustdoc para as traits públicas e `#![warn(missing_docs)]`.
- **Diagnóstico:** `--log-level`, `--log-format {pretty,json}` e `veil doctor` (globals do Wayland, plugins do GStreamer, VA-API, permissões do socket, configuração).
- **Depois do lançamento:** um esquema de labels para a triagem de bugs, uma cadência de lançamentos (lançamentos de correção para as correções), uma política de depreciação para o alias `--daemon` e os relatos de compatibilidade dos usuários.
