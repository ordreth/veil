---
title: "Problemas conhecidos"
description: "O que o veil não consegue corrigir hoje, e por quê."
group: "Reference"
order: 9
source: "docs/known-issues.md"
source_hash: "2771f905064739ee2ba9fb66ae6e8278f10ee2ac749eef438511763e055283af"
---

Este arquivo registra problemas que o veil não consegue (ou deliberadamente não quer) resolver hoje, o que foi medido, quais opções foram consideradas e o que foi decidido. Ele existe para que quem voltar a um destes problemas mais tarde não precise redescobri-lo.

Cada entrada tem a data da última verificação. Verifique de novo antes de confiar nela: o GStreamer e os compositores mudam.

---

## 1. Uma mudança de monitor enquanto um vídeo é reproduzido derruba o daemon (bug do `waylandsink` do GStreamer)

**Situação:** corrigido depois da 1.0.0-beta.1 (2026-11-09): o veil não usa mais o `waylandsink` (opção E, veja "Corrigido: o sink próprio do veil" no fim desta entrada). O bug em si continua no GStreamer e não foi relatado ao projeto original.
**Afetava:** os papéis de parede em vídeo da 1.0.0-beta.1. Imagens estáticas e a cor de fundo sólida nunca foram afetadas.
**Última verificação:** 2026-11-09, sway 1.12 headless, GStreamer 1.28.7: nenhuma das mudanças das tabelas abaixo encerra mais o daemon (`output_changes_while_videos_play_leave_them_playing` em `tests/headless.rs`). Ainda não verificado no Hyprland. O que vem a seguir, até "Corrigido", é o histórico.

### Sintoma

O daemon morre com SIGSEGV (código de saída 139 em um shell). Não há encerramento limpo: nenhuma linha de log do veil, e o arquivo do socket de IPC fica para trás (inofensivo: o próximo daemon o substitui, e `veil set` informa que nenhum daemon está rodando). O core dump mostra a falha na thread chamada `GstWlDisplay`:

```text
#0 g_mutex_lock            (libglib-2.0.so.0)
#1 output_done             (libgstwayland-1.0.so.0)
#2 ffi_call_unix64 ...
#5 wl_closure_invoke       (libwayland-client.so.0)
#7 wl_display_dispatch_queue_pending
#8 gst_wl_display_thread_run (libgstwayland-1.0.so.0)
```

### O que provoca

Medido em um sway 1.12 headless com um vídeo em reprodução:

| Configuração | Ação | Resultado |
|---|---|---|
| Uma saída | Mudar sua escala | **derruba o daemon** |
| Uma saída | Desativá-la e ativá-la de novo (10 vezes) | funciona: o vídeo recomeça a cada vez |
| Uma saída | O compositor termina ou é morto | saída limpa, código de saída 5 (`EXIT_COMPOSITOR_GONE`; não é este bug) |
| Duas saídas, vídeo na A | Desativar a B | **derruba o daemon** |
| Duas saídas, vídeo na A | Mudar a escala da B | **derruba o daemon** |
| Duas saídas, vídeo na A | Mover a B (mudar sua posição) | **derruba o daemon** |
| Duas saídas, vídeo na A | Conectar uma terceira saída | **derruba o daemon** |
| Duas saídas, vídeo na A | Desativar a própria A | **derruba o daemon** |
| Duas saídas | Nada muda | funciona |

Em resumo: com um único monitor, desconectá-lo e reconectá-lo não causa problema. Com mais de um monitor, qualquer mudança no layout dos monitores mata um vídeo em reprodução, porque o compositor então reenvia o estado dos monitores que ficam.

Medido de novo com suporte a vários monitores (2026-10-14, sway 1.12 headless, duas saídas, cada saída com uma superfície própria):

| Configuração | Ação | Resultado |
|---|---|---|
| Imagens nas duas saídas | Acrescentar uma terceira saída, desativar uma, ativá-la, desconectar uma | funciona: a nova saída recebe o papel de parede, as outras ficam inalteradas (0 pixels diferentes) |
| Vídeo nas duas saídas | Acrescentar uma terceira saída | **derruba o daemon** |
| Vídeo nas duas saídas | Desativar a segunda saída | **derruba o daemon** |
| Vídeo na primeira, imagem na segunda | Desativar a segunda | **derruba o daemon** |
| Imagem na primeira, vídeo na segunda | Desativar a primeira | **derruba o daemon** |
| Vídeo na primeira, segunda desativada antes | Ativar a segunda | **derruba o daemon** |

O mesmo core dump (`output_done` na thread `GstWlDisplay`). Nada mudou em comparação com uma superfície: um único pipeline já era morto por uma mudança em qualquer outra saída, porque todo sink se vincula a toda saída. O que mudou foi a frequência com que isso é encontrado, já que o daemon agora desenha em toda saída por padrão: um papel de parede em vídeo em uma configuração com vários monitores é reproduzido em cada monitor, e a primeira mudança de layout encerra todos eles.

Medido quando os vídeos não vistos passaram a ser liberados (2026-10-15, sway 1.12 headless, uma saída, `swaymsg output HEADLESS-1 power off` e `power on`):

| Configuração | Ação | Resultado |
|---|---|---|
| Vídeo, sem a liberação de vídeos não vistos | Desligar a saída, depois ligar | desligar: funciona (o vídeo continua decodificando); ligar: **derruba o daemon** |
| Vídeo, com a liberação | Desligar por 4 s, depois ligar | funciona: o vídeo foi parado após 2 s (não estava sendo exibido) e recomeça ao ligar |
| Vídeo, com a liberação | Desligar e ligar de novo em 1 s | **derruba o daemon** (o vídeo ainda não tinha sido parado) |
| Vídeo | Bloquear a sessão (`swaylock`), desbloquear | funciona |

Ao ligar, o sway reenvia a geometria, a escala e o `done` da saída a todo cliente, e o `wl_output` do próprio sink recebe o segundo `done`. Um monitor que é posto para dormir por um daemon de ociosidade (DPMS) e acordado de novo derruba, portanto, o daemon no sway, sempre que um vídeo ainda estiver vivo naquele momento.

Foi reproduzido apenas no sway. Pela causa abaixo, deveria acontecer em todo compositor, inclusive no Hyprland, sempre que o compositor envia um segundo `wl_output.done` para uma saída. Quais ações fazem o Hyprland fazer isso **não** foi testado (a sessão do desenvolvedor foi deixada em paz de propósito).

### Causa

O `waylandsink` compartilha a conexão Wayland do veil. Ele cria seu próprio objeto auxiliar (`GstWlDisplay`), que se vincula a todo `wl_output` que o compositor anuncia e trata seus eventos em uma thread própria.

Em `gst-libs/gst/wayland/gstwldisplay.c`:

- `registry_handle_global` guarda um ponteiro de volta em cada objeto de saída: `g_object_set_data (G_OBJECT (output), "display", self);`
- `output_done` o lê com `g_object_steal_data (G_OBJECT (output), "display")`, que o *remove*, e depois o usa sem verificar `NULL` (`gst_wl_display_get_instance_private (self)`, depois `g_mutex_lock (&priv->outputs_mutex)`).

Ou seja, o código presume exatamente um evento `done` por saída. O compositor envia `done` de novo depois de toda mudança da saída (protocolo `wl_output`: "sent after all other properties have been sent"), e o segundo desreferencia `NULL`.

Uma correção precisa tornar `output_done` seguro para ser chamado mais de uma vez por saída. Trocar `g_object_steal_data` por `g_object_get_data` sozinho não é obviamente suficiente, porque o tratador também (re)insere a saída em uma tabela hash que detém uma referência; verifique isso antes de escrever um patch.

### Por que o veil não consegue evitar

- Os eventos vão direto da libwayland para a thread do GStreamer. Nenhum código do veil roda no meio, então não há nada a interceptar, e o veil fica sabendo da mudança no mesmo instante.
- O `waylandsink` se vincula às saídas por conta própria. Não há propriedade para desligar isso, e o sink precisa usar a conexão do veil (o vídeo é uma subsuperfície da superfície do veil), então não é possível dar a ele uma conexão filtrada.
- O objeto auxiliar vive tanto quanto o sink, mesmo no estado `Null`. Só destruir o sink o faz parar.

### O que o veil faz a respeito

- O pipeline de vídeo (e com ele o sink e sua thread) existe apenas enquanto há uma superfície onde reproduzir. Quando a saída do papel de parede some, `VideoEngine::detach` descarta o pipeline antes de a superfície ser destruída, e um novo pipeline é construído quando a saída volta. Nada pode falhar enquanto a saída está ausente.
- Um vídeo cujos quadros o compositor recusou por 2 segundos (`--release-unseen-after`) é parado e seu pipeline descartado, e construído de novo quando a saída é desenhada outra vez. Um monitor que ficou desligado por mais tempo que isso não tem sink para matar quando volta. Isso não é uma correção: não cobre um monitor desligado e ligado em menos de 2 segundos, um vídeo pausado pela pausa automática (que não está sendo visto), `--release-unseen-after 0`, nem qualquer outra mudança de saída.
- O papel de parede de cada saída é salvo (`$XDG_STATE_HOME/veil/state.json`), e um daemon iniciado de novo sem `--input` mostra os mesmos papéis de parede. Verificado em um sway headless em 2026-10-16: uma mudança de escala da saída em que um vídeo era reproduzido matou o daemon (código de saída 139), e o daemon iniciado de novo à mão mostrou a imagem e o vídeo que tinha.
- A unidade de usuário do systemd (`contrib/systemd/veil.service`, `Restart=on-failure`, `RestartSec=2`) reinicia o daemon após a falha, e ele volta com esses papéis de parede: a opção D está feita. Verificado em um sway headless sob o gerenciador de usuário do systemd do desenvolvedor: a mudança de escala da saída em que um vídeo era reproduzido matou o daemon (`code=dumped, status=11/SEGV`), e 2.9 s depois a unidade estava ativa de novo com o vídeo em reprodução. Isso exige que o daemon rode sob a unidade; iniciado à mão, ele ainda precisa ser iniciado de novo à mão. Um teste disso ainda está por escrever ([`testing.md`](https://github.com/ordreth/veil/blob/stable/docs/testing.md), "Checks to be turned into tests").
- A limitação está declarada no README ("Known limitations"), na documentação do módulo `src/engine/video.rs` e aqui.

### Opções consideradas

| Opção | O que dá | Por que não foi feita (ainda) |
|---|---|---|
| **A. Aceitar e documentar** | Nada quebra que já não estivesse quebrado. As imagens são totalmente robustas. | *Escolhida.* |
| **B. Aplicar um patch no `gst-plugins-bad` no flake do Nix** | A falha desaparece nas compilações do Nix (dev shell e pacote). | O conjunto de plugins do GStreamer passa então a ser compilado a partir do código-fonte: uma primeira compilação longa localmente e em toda execução do CI sem um cache binário. As outras distribuições continuam falhando. Poderia ser combinada com o wrapper do Nix. |
| **C. Relatar o bug ao projeto original** | Uma correção para todos, com o tempo. | Recusada por ora pelo mantenedor (2026-10-10). Nenhum relato existente no projeto original foi encontrado na época. |
| **D. Reiniciar após a falha** | O papel de parede volta sozinho em um ou dois segundos. | Precisa de um supervisor (a unidade do systemd com `Restart=on-failure`) e da persistência do último papel de parede; sem isso, o daemon reiniciado mostra o papel de parede da inicialização. Os dois existem; juntos, são a mitigação real mais barata. Um supervisor embutido no veil (um processo pai que reinicia um worker) faria o mesmo sem o systemd, ao custo de código de gerenciamento de processos. |
| **E. Substituir o `waylandsink`** | Elimina o bug e toda esta classe de dependência do `waylandsink` (veja também o problema 2). | Grande: o veil anexaria ele mesmo os buffers de vídeo (por exemplo `appsink` mais buffers DMABUF/SHM em uma subsuperfície própria, ou um caminho GL). Sobrepõe-se à investigação de cópia zero (entrada 7), que é o lugar natural para decidir isso. |
| **F. Contornar dentro do processo** | (nada limpo) | As únicas maneiras são mexer nos objetos privados do GStreamer ou interpor símbolos da GLib. Rejeitada por ser frágil demais. |

### Decisão (2026-10-10)

Opção A. Fazer tudo o que é possível do lado do veil, documentar o resto e não relatar ao projeto original por ora. Substituir o `waylandsink` (opção E) pode ser reconsiderado mais tarde, provavelmente junto com a questão da cópia zero (entrada 7). A opção D está feita (2026-10-16): a unidade do systemd e os papéis de parede salvos.

### Quando revisar

- Uma versão do GStreamer muda `output_done` em `gstwldisplay.c` (verificar se a linha `g_object_steal_data` ainda está lá).
- ~~A investigação de cópia zero~~: decidido em 2026-10-17: o veil mantém o `waylandsink` na 1.0. As medições não tornaram necessário substituí-lo: o que um vídeo custa é a conversão de cor, de que precisa qualquer sink que mostre os quadros da NVIDIA por memória compartilhada (entrada 7). A opção E fica para depois da 1.0; para a 1.0 a resposta a este problema é a opção D (a unidade do systemd e os papéis de parede salvos), ainda a verificar à mão no Hyprland.
- ~~Suporte a vários monitores~~: feito; medido de novo (veja a segunda tabela acima). Como esperado, o bug é o mesmo e atinge mais configurações, porque agora todo monitor reproduz o vídeo. É até agora o argumento mais forte para a opção B, D ou E.
- Os papéis de parede salvos e a unidade do systemd estão feitos (2026-10-16, veja "O que o veil faz a respeito"): sob a unidade do systemd, uma falha se recupera sozinha. Um teste disso ainda está por escrever ([`testing.md`](https://github.com/ordreth/veil/blob/stable/docs/testing.md), "Checks to be turned into tests").
### Corrigido: o sink próprio do veil (2026-11-09)

Opção E. O veil mostra ele mesmo os quadros de um vídeo: `PlaneSink` (`src/engine/video/sink.rs`), um sink de vídeo do GStreamer próprio do veil, converte cada quadro em um buffer `wl_shm` de uma subsuperfície que o código Wayland do veil cria (`src/wayland/plane.rs`). Os objetos da subsuperfície são criados na fila de eventos do próprio veil, então seus eventos são tratados pelo loop de eventos, e nada vincula um `wl_output` além do código do próprio veil. Com isso a falha desapareceu, e também o motivo para o reinício sob a unidade do systemd e para a maior parte da liberação de vídeos não vistos (os dois ficam: a unidade para qualquer outra falha, a liberação porque economiza a CPU de um vídeo que ninguém vê).

O que foi preciso fazer que o `waylandsink` fazia sozinho: o ritmo dos frame callbacks (um quadro que chega enquanto o compositor não aceitou o anterior é descartado e contado, então o `wayland::unseen` continua funcionando), o quadro de preroll (exibido em `Paused`, e um preroll nunca pode relatar um descarte, senão o `basesink` nunca termina a mudança de estado: descoberto quando um vídeo pausado atrás de uma janela em tela cheia nunca mais voltou a tocar), e a posição da subsuperfície (estado do seu pai, com commit feito pelo loop de eventos quando o sink informa um movimento).

O que se ganhou além disso: a conversão e o recorte de `fill` e `center` acontecem em uma única passada, direto na memória do compositor, e nenhum pixel recortado é convertido (entrada 7); o loop de eventos acorda cerca de metade das vezes enquanto um vídeo é reproduzido (`measurements.md`, "veil's own sink"). Reduzir na CPU um quadro maior que o seu lugar foi tentado e abandonado: o conversor do GStreamer é mais lento quando escala do que quando converte no tamanho do próprio quadro (`src/engine/video/sink.rs`). O que custa: memória, em um compositor que retém o quadro que exibe até o próximo substituí-lo (um sway headless): o sink então passa de dois buffers para três, cerca de 100 MB a mais que o `waylandsink` para um vídeo 4K. O que se perdeu: quadros em dmabufs. O sink aceita só memória de sistema; na NVIDIA isso era tudo o que havia (entrada 7), em Intel e AMD com VA-API um decodificador que poderia entregar dmabufs ao compositor agora entrega memória de sistema ao veil, o que nunca foi medido em nenhum dos casos (sem esse hardware).

O teste que esperava a falha virou `output_changes_while_videos_play_leave_them_playing`: duas saídas reproduzindo o mesmo clipe, uma mudança de escala, um movimento, uma desativação e uma ativação, um desligar e ligar em menos de um segundo, e uma saída nova; o daemon continua rodando e todo vídeo se move depois.

### Como reproduzir (1.0.0-beta.1)

Este é o procedimento usado para encontrar o bug, escrito à mão (o sway e os plugins do GStreamer do veil precisam estar disponíveis, por exemplo `nix shell nixpkgs#sway` dentro do `nix develop`):

```bash
# A private headless compositor, so the real session is not touched.
# XDG_RUNTIME_DIR must be a short path (Unix socket path limit).
export XDG_RUNTIME_DIR=/run/user/$UID/veil-test && mkdir -p -m 700 "$XDG_RUNTIME_DIR"
env -u WAYLAND_DISPLAY WLR_BACKENDS=headless WLR_RENDERER=pixman \
    WLR_LIBINPUT_NO_DEVICES=1 LIBSEAT_BACKEND=noop sway &
export WAYLAND_DISPLAY=wayland-1 SWAYSOCK=$(ls "$XDG_RUNTIME_DIR"/sway-ipc.*)

./target/release/veil daemon --input some-video.webm &
sleep 3
swaymsg output HEADLESS-1 scale 2      # the daemon segfaults here
coredumpctl info veil                  # shows output_done on thread GstWlDisplay
```

---

## 2. Um pipeline de vídeo não pode passar para outra superfície

**Situação:** corrigido (2026-11-20). Desde que o veil mostra os seus vídeos ele mesmo (2026-11-09, entrada 1), um pipeline pode passar para outra superfície. Quando uma saída some, o pipeline do seu vídeo é mantido, pausado no quadro em tela e sem o seu plano (`VideoEngine::set_aside`), e a próxima superfície da saída o recebe de volta com um seek para esse quadro: o vídeo continua de onde estava (`a_video_goes_on_where_it_was_when_its_output_comes_back` em `tests/headless.rs`). Um pipeline mantido retém sua memória e suas threads, por isso é liberado depois de `--release-unseen-after` (2 s por padrão), como um vídeo não visto, e uma saída que fica fora por mais tempo recebe o vídeo do início. O que vem a seguir é o histórico.
**Última verificação:** 2026-10-10, GStreamer 1.28.7.

O `waylandsink` não pode trocar seu handle de janela: um segundo `set_window_handle` apenas registra "changing window handle is not supported". Ele também mantém seus objetos de janela e de display até o elemento ser finalizado, mesmo no estado `Null`.

Consequências no veil:

- Um pipeline atende exatamente uma superfície (`Player` em `src/engine/video.rs`). Quando a superfície é perdida, o pipeline é descartado e reconstruído no próximo `attach`. O vídeo então começa do início.
- Isso não conflita com a regra "nunca recriar o pipeline nas repetições do vídeo": as repetições continuam usando seek. Uma reconstrução acontece apenas quando a própria superfície some.

---

## 3. A memória não é devolvida depois de trocas de vídeo (arenas do alocador da glibc)

**Situação:** mitigado onde o veil é empacotado (opção C): com uma arena do alocador, que o pacote Nix e a unidade do systemd definem (decidido em 2026-10-16, medido em 2026-10-17), o heap não sobe ao longo de 500 trocas, e a reprodução custa o mesmo. Aberto para um binário iniciado sem a variável, que recebe o padrão da glibc e o crescimento abaixo. A B não é necessária. Veja "Medido (2026-10-17)".
**Afeta:** um daemon que substituiu um vídeo por outro, ou cuja saída foi desconectada e reconectada enquanto um vídeo era reproduzido. Também toda vez que um vídeo é parado porque ninguém o está vendo (uma tela bloqueada, um monitor desligado) e iniciado de novo: um pipeline construído por bloqueio ou por desligamento do monitor, por saída. Um daemon que só exibe imagens não é afetado. Um vídeo que apenas se repete não reconstrói nada, e não reteve nada em dez minutos de repetições; dias ficam a cargo do teste de longa duração de 24 horas do checklist de lançamento ([`roadmap.md`](/apps/veil/docs/roadmap)).
**Última verificação:** 2026-10-17, glibc 2.44, GStreamer 1.28.7, compilação de lançamento, 12 threads de CPU, sway headless. Medido pela primeira vez em 2026-10-13; visto de novo depois de acrescentada a troca atômica de engines (200 trocas de vídeo: 191 MB para 306 MB).

Esta entrada descrevia o vazamento de threads dos pipelines substituídos. Esse vazamento foi corrigido: a thread do bus de um pipeline agora recebe join quando o pipeline é descartado. As threads e os descritores de arquivo ficam estáveis ao longo de 600 trocas de vídeo. O que é descrito aqui foi encontrado ao verificar essa correção, e existe com ela e sem ela.

### Sintoma

A memória residente (RSS) do daemon sobe com as primeiras trocas de vídeo e não volta, nem quando uma imagem substitui o vídeo. Depois, continua subindo devagar.

Medido em um sway 1.12 headless (1920x1080), alternando `veil set` entre um clipe VP8 de 1280x960 e um de 2560x1080:

| Estado | RSS |
|---|---|
| Daemon iniciado com uma imagem, nenhum vídeo reproduzido ainda | 18.6 MB |
| Imagem de novo após 1 vídeo | 99.8 MB (64 MB são as bibliotecas do GStreamer e dos codecs, que permanecem mapeadas) |
| Imagem de novo após 11 trocas de vídeo | 183 MB |
| Imagem de novo após 111 | 244 MB |
| Imagem de novo após 301 / 511 / 601 (duas execuções) | 261 / 263 / 330 MB |
| Um vídeo em reprodução, no início | 190 MB |
| Um vídeo em reprodução, após 200 trocas | 336 MB |
| Um vídeo em reprodução, após 10 ciclos de desconectar e reconectar sua saída | 238 MB, a partir de 175 MB |

### Causa

É memória livre que o alocador da glibc retém, não memória que algo ainda use:

- O rastreador de vazamentos do próprio GStreamer não encontra nenhum objeto que sobreviva a uma troca (os mesmos 224 objetos do registro estão vivos após 1, 11 e 111 trocas, com uma imagem na tela).
- Com uma arena do alocador (`MALLOC_ARENA_MAX=1`), a mesma execução fica estável: 116, 124, 126 e 121 MB após 11, 111, 311 e 511 trocas.
- O `malloc_trim(0)`, chamado por um depurador após 100 trocas com uma imagem na tela, levou o daemon de 226 MB para 118 MB.

Um pipeline de vídeo roda cerca de sete threads de streaming, e toda troca as inicia de novo. A glibc dá às threads arenas próprias (até oito por thread de CPU), e os blocos grandes que um decodificador libera em uma delas só voltam ao sistema quando por acaso estão no fim daquela arena. Ao longo de muitas trocas, mais arenas entram em uso e cada uma retém o que um dia precisou.

### Opções consideradas

| Opção | O que dá | O que custa |
|---|---|---|
| **A. Aceitar e documentar** | Nada muda. A memória é limitada em teoria (um número fixo de arenas), mas nenhum limite foi atingido em 601 trocas. | A meta de "nenhum crescimento de memória" deixa então de ser atingida para quem troca papéis de parede em vídeo com frequência (uma playlist, planejada para depois da 1.0). O item do checklist de lançamento "RSS estável ao longo de 500 trocas de papel de parede" não pode passar. |
| **B. Chamar `malloc_trim(0)` depois que um pipeline é desmontado** | Devolve o que está livre: 108 de 226 MB na medição. | Uma chamada `unsafe` à glibc, que só existe lá (precisa ficar de fora na musl). O veil só permite `unsafe` para passar ponteiros brutos do Wayland, em um módulo (`engine::video`), então essa regra teria de mudar. Quanto a chamada demora não foi medido; ela não deve rodar no loop de eventos. Um vídeo substituído é desligado em uma thread própria (`veil-teardown`, `src/wayland/retire.rs`), que é o lugar natural para ela. |
| **C. Menos arenas** | Com `MALLOC_ARENA_MAX=2`: 235 MB em vez de 337 MB após 300 trocas com um vídeo em reprodução. Com `1`: estável, veja acima. | Como variável de ambiente, só ajuda onde o veil é iniciado por algo que a define (a unidade do systemd, um wrapper). Defini-la por dentro exige `mallopt`, uma chamada `unsafe` como a B. As threads do decodificador passam então a compartilhar locks do alocador; o efeito na reprodução não foi medido. Ela desacelera o crescimento e não devolve memória. |
| **D. Outro alocador para o código Rust** (`#[global_allocator]`) | Nada: a memória em questão é alocada pelo GStreamer e pelos decodificadores por meio da biblioteca C, não pelo código Rust. | |
| **E. Manter um pipeline e trocar seu arquivo** | Menos objetos são reconstruídos em uma troca de vídeo para vídeo. | Não medido, e as threads de streaming ainda são iniciadas de novo para cada arquivo, então as arenas podem se encher do mesmo jeito. Não foi tentado. |
| **F. Um limiar de mmap fixo** (`MALLOC_MMAP_THRESHOLD_`) | Não medido. A glibc eleva seu limiar para `mmap` cada vez que um bloco grande é liberado, então depois dos primeiros quadros os buffers de um decodificador vêm das arenas; um limiar fixo desliga isso, e blocos acima dele são devolvidos quando liberados. | Uma variável de ambiente como a C, com o mesmo alcance. Toda alocação grande vira uma chamada de sistema. |

Toda sessão bloqueada e todo monitor desligado reconstrói um pipeline por saída quando volta, então a memória cresce com o uso diário e não só com trocas manuais. A decisão foi, portanto, tomada antes dos testes de longa duração.

### Decisão (2026-10-16, o mantenedor)

**C agora, B só se os testes de longa duração mostrarem que a C não basta.**

- `MALLOC_ARENA_MAX=2` é definido onde o veil é iniciado: o wrapper do Nix o define a menos que a variável já esteja definida (`--set-default` em `nix/package.nix`; a verificação `plugins` do flake confere os dois casos), e a unidade do systemd tem `Environment=MALLOC_ARENA_MAX=2` (`tests/systemd.rs` confere). Nenhum código mudou, e o `unsafe` fica onde estava. Um binário iniciado de outro modo (`cargo build`, os testes headless, `scripts/bench/` sem a variável) roda com o padrão da glibc.
- O teste de longa duração de trocas mede o daemon com `2` (o que é distribuído), `1` e o padrão da glibc, e a F se houver tempo, junto com o que cada um custa à reprodução (quadros exibidos, CPU). Se nenhum deles ficar estável, a B entra: `malloc_trim(0)` na thread `veil-teardown`, e um segundo lugar com permissão para `unsafe`, para essa única chamada.
- Por que não a B de imediato: ela alarga a regra do `unsafe` por algo que uma variável de ambiente pode resolver, e com `1` a execução ficou estável. Por que `2` e não `1`: o `1` faz as sete ou mais threads do decodificador compartilharem um único lock do alocador, e o que isso custa à reprodução não foi medido; o teste de longa duração mede isso e pode mudar o número.
- O que a C não dá: a memória que a glibc já retém não é devolvida, então com `2` o crescimento é mais lento, não eliminado (235 MB em vez de 337 MB após 300 trocas). Usuários que compilam o veil por conta própria e não usam a unidade não recebem nada disso.

### Medido (2026-10-17): o número virou 1

O teste de longa duração de trocas (`scripts/bench/soak.sh switch`: 500 `veil set`, imagens e vídeos em alternância, H.264 1280x960 e VP9 2560x1080 pelos decodificadores da NVIDIA) e o `bench.sh` para o que cada configuração custa à reprodução; os números estão em [`measurements.md`](/apps/veil/docs/measurements), "Testes de longa duração e o alocador". O heap (`RssAnon`) com uma imagem na tela:

| `MALLOC_ARENA_MAX` | Após 1 vídeo | Após 250 vídeos | Nas trocas de 100 a 500 |
|---|---|---|---|
| 1 | 42.5 MiB | 42.2 MiB | estável |
| 2 | 41.2 MiB | 73.0 MiB | estável a partir da décima troca |
| padrão da glibc (96 arenas) | 41.4 MiB | 129.1 MiB | subindo (+6.25 KiB por troca) |
| padrão com `MALLOC_MMAP_THRESHOLD_=131072` (F) | 22.4 MiB | 107.9 MiB | subindo (+0.86 KiB por troca) |

A reprodução custou o mesmo com os três números, dentro do ruído de uma execução: a CPU e os quadros por segundo de um clipe H.264 1080p60 e de um 2160p60 com o decodificador da NVIDIA, do clipe 2160p60 por software (150 a 152 % de um núcleo, 60 quadros por segundo), e de dois vídeos ao mesmo tempo em duas saídas (dois streams 4K por software: 363.5, 361.9 e 357.6 % com o padrão, 2 e 1, 120 quadros por segundo cada). O `veil set` levou o mesmo tempo (110 ms para uma imagem, 176 ms para um vídeo, a mesma CPU por troca).

Assim, as duas condições que a decisão nomeou foram atendidas: a C basta (a B não é necessária), e o `1` é estável e não custa nada mensurável à reprodução. **O veil é distribuído com `MALLOC_ARENA_MAX=1`** (o wrapper do Nix, `--set-default`, e `Environment=MALLOC_ARENA_MAX=1` na unidade do systemd; a verificação `plugins` do flake e `tests/systemd.rs` os conferem). Um usuário ainda pode definir outro número. A F sozinha não basta e custa um pouco de tempo por troca; ela não foi combinada com um limite de arenas, que não precisa de mais nada.

O que resta: um binário iniciado sem a variável (`target/release/veil`, um pacote de distribuição que não a define) recebe o padrão da glibc e o crescimento da tabela; ele é limitado pelo número de arenas, mas nenhum limite foi atingido em 600 trocas. E o teste de longa duração rodou em um sway headless, com os quadros em memória compartilhada: uma sessão real com buffers dmabuf pode alocar de forma diferente (o teste de longa duração de 24 horas do checklist de lançamento, [`roadmap.md`](/apps/veil/docs/roadmap)).

### Como reproduzir

`scripts/bench/soak.sh switch --arenas N` (com `default` para o da própria glibc), ou à mão em um sway headless como na entrada 1: inicie `veil daemon --input image.png`, depois alterne `veil set` entre dois arquivos de vídeo algumas centenas de vezes, defina a imagem de novo e leia `VmRSS`, `RssAnon` e `Threads` de `/proc/<pid>/status`. Repita com `MALLOC_ARENA_MAX=1` no ambiente do daemon. Um daemon iniciado a partir do `nix build` ou pela unidade do systemd já tem `MALLOC_ARENA_MAX=1`; para ver o padrão da glibc, defina a variável como `8` vezes o número de threads de CPU, ou inicie `target/release/veil` diretamente. Para o rastreador de vazamentos, inicie o daemon com `GST_TRACERS=leaks GST_LEAKS_TRACER_SIG=1 GST_DEBUG=GST_TRACER:7` e envie a ele SIGUSR1 (só depois do primeiro vídeo: antes disso o GStreamer não está inicializado, e o sinal encerra o daemon).

---

## 4. Um papel de parede fica com sua saída e não se move

**Situação:** por projeto. A regra é mantida por saída desde o suporte a vários monitores.
**Decidido:** 2026-10-10; adaptado em 2026-10-14 para o suporte a vários monitores.

Toda saída em que o daemon desenha tem um papel de parede próprio, lembrado **pelo nome da saída** (`OutputKey` em `src/wayland/binding.rs`, o `Screen` em `src/wayland/screen.rs`). Quando uma saída some, sua superfície é liberada e seu papel de parede é mantido (desanexado, sem reter pixels nem pipeline); quando uma saída com o mesmo nome é anunciada de novo, o papel de parede é recolocado nela. As outras saídas não são tocadas.

Considerado e rejeitado: mover um papel de parede para outra saída conectada quando a sua desaparece. Isso faria os papéis de parede pularem entre monitores (e não pularem de volta). Antes do suporte a vários monitores, o daemon controlava uma única saída, e o preço dessa regra era que um laptop iniciado acoplado não mostrava nada depois de desacoplado. Com isso, essa questão desaparece: sem `--output`, toda saída, mesmo uma conectada depois, recebe uma superfície. Uma saída em que o daemon ainda não desenhou recebe o papel de parede padrão: o do último `veil set` para toda saída cujo arquivo pôde ser carregado, ou então o `--input`. Ela mostra `--bg-color` enquanto esse arquivo é carregado fora do loop de eventos.

Com `--output` (repetível), apenas as saídas nomeadas ali recebem uma superfície. Um `veil set --output X` para uma delas é aceito antes de X estar conectada: o papel de parede é mantido para ela.

Se o compositor não informa nenhum nome de saída, a saída não pode ser reconhecida quando volta: seu papel de parede termina com sua superfície, e ela é tratada como uma saída nova quando é anunciada de novo. Ela não pode ser nomeada em `veil set --output`; um `veil set` para todas as saídas a inclui.

Nomes de `--output` que não existem são um erro **apenas na inicialização**, e só se nenhum deles existir (provavelmente um erro de digitação). Se alguns existirem, os ausentes são registrados no log e esperados. Depois que uma saída foi vinculada uma vez, sua ausência nunca é um erro.

---

## 5. Um vídeo que cobre toda a superfície fica parado por um momento a cada repetição

**Situação:** não reproduzido desde 2026-10-16, com nenhuma compilação, nem com a que o encontrou; mantido aberto como registro. Nenhum teste o procura, e o Hyprland, onde ele mais importaria, não é coberto por testes ([`testing.md`](https://github.com/ordreth/veil/blob/stable/docs/testing.md), "Not covered by any test"); ele é reaberto se for visto no uso. Encontrado em 2026-10-13, e mais antigo que isso: a compilação anterior à adição da recuperação de erros se comportava da mesma forma então.
**Afeta:** vídeos em `fill` (o padrão) e `stretch`, e qualquer modo em que o vídeo por acaso cubra a superfície por completo. Não `fit` com barras nem `center` com uma borda.
**Última verificação:** 2026-10-16, sway 1.12 headless (renderizador pixman, quadros por `wl_shm`), GStreamer 1.28.7: não estava lá. **Não verificado no Hyprland nem em hardware real.**

### Sintoma

Cada vez que o vídeo recomeça do início, a imagem não se mexe por cerca de 0.7 segundo. Com uma repetição curta, isso é bem visível.

Medido com 150 capturas de tela tiradas em sequência ao longo de 20 segundos, de um clipe de 3 s cujo conteúdo se move desde o primeiro quadro (seus quadros decodificados são todos diferentes):

| Modo | Trechos com a imagem inalterada |
|---|---|
| `fill` (o vídeo cobre a superfície) | seis, um a cada repetição: de 0.67 a 0.82 s cada |
| `fill`, uma compilação anterior | seis, de 0.67 a 0.81 s cada |
| `fit` (barras da cor de fundo à esquerda e à direita) | nenhum |
| `center` com um clipe menor | nenhum |

### O que se sabe

- O pipeline não percebe nada. Ele volta a `Playing` 5 ms depois do seek, o fim da execução chega exatamente um comprimento de clipe depois, e o sink conta todo quadro como renderizado e nenhum como descartado.
- Então os quadros se perdem entre o sink e a tela. Um mecanismo sugerido antes, o de que o sink pula um quadro enquanto ainda espera o frame callback do compositor para o anterior e o conta como renderizado, é improvável: no código-fonte da 1.28.7 um quadro substituído desse modo é informado como descartado (`gst_wl_window_render_hdr` retorna false, o sink retorna `GST_BASE_SINK_FLOW_DROPPED`), e o `dropped` foi visto crescer 30 por segundo enquanto uma sessão bloqueada impedia o compositor de responder. Aqui ele fica em zero.
- O sway não envia frame callbacks à superfície do veil enquanto uma subsuperfície de vídeo opaca a cobre inteira (medido duas vezes: nenhum em 3 s, enquanto o sink recebeu 180 para a sua própria subsuperfície). O loop do próprio veil daquela época (removido por um dia, veja a entrada 6), que fazia commit dessa superfície uma vez por frame callback, ficava portanto parado exatamente nos modos que mostravam este problema, e rodava nos modos que não o mostravam.
- O que não foi estabelecido: por que isso afeta só depois do seek com flush e não durante a reprodução normal, e o que encerra a espera após 0.7 s.

### Não reproduzido (2026-10-16)

Foi medido de novo antes de qualquer coisa ser alterada, com capturas de tela tiradas em sequência (26 por segundo como PPM, 8 por segundo como PNG, como na primeira medição) ao longo de 8 a 20 segundos, isto é, de três a seis repetições por execução, e não encontrou nenhum trecho com a imagem inalterada maior que 100 ms em nenhum destes:

- uma compilação de medição inicial e a compilação com que foi encontrado pela primeira vez (commit `9b41671`, compilada de novo para isso);
- o sway do shell `headless` do flake e o do `nix shell nixpkgs#sway` (ambos 1.12, compilações diferentes), e o `grim` do host assim como o do shell;
- os decodificadores `nvvp8dec` (o padrão na máquina de referência), `vp8dec` e `avdec_vp8` para VP8, `nvh264dec`, `nvh265dec` e `nvvp9dec` para os clipes de referência de `scripts/bench/clips.sh`;
- `fill` com clipes do tamanho da saída e de outros tamanhos (ampliados, cortados), `fit` e `center`, nas escalas de saída 1, 1.5 e 2;
- um clipe SMPTE de 1.9 MB como o da primeira medição, e a bola em movimento.

O que difere de 2026-10-13 não pode ser estabelecido agora: o sway da primeira medição veio do `nixpkgs` do registro, que avançou desde então, e o registro do GStreamer que o daemon usou então era o compartilhado, que depois se descobriu quebrado (2026-10-15), então o decodificador pode ter sido outro. Também foi tentado sem o commit da superfície do veil a cada quadro, a ligação suspeita: nenhuma parada também, em `fill`, `fit` e `center`. (Essa mudança foi desfeita porque o Hyprland precisa dos commits: entrada 6.)

### Opções

Se voltar: medir primeiro (o procedimento abaixo), depois comparar `fill` com `fit`: o commit do veil a cada quadro (entrada 6) roda em `fit` e fica parado em `fill` no sway, que é o único mecanismo conhecido que difere entre os modos. Uma solução alternativa para um usuário que o vir: `--mode fit` em um vídeo cuja proporção difere da do monitor.

### Como reproduzir

Em um sway headless como na entrada 1, inicie `veil daemon --input clip.webm` com um clipe curto de algo que se mova o tempo todo (`videotestsrc pattern=ball` dá um), tire capturas de tela o mais rápido que o `grim` permitir ao longo de algumas repetições e compare cada uma com a anterior (`magick compare -metric AE`). Repita com `--mode fit` e um clipe mais estreito que a saída.

---

## 6. O Hyprland precisa que a superfície do veil receba commit a cada quadro enquanto um vídeo é reproduzido

**Situação:** contornado (o loop de commits fica); a causa não é conhecida. **Encontrado:** 2026-10-16, pelo mantenedor, em sua sessão do Hyprland, com uma compilação que não fazia mais commit da superfície a cada quadro. **Afeta:** qualquer mudança no loop de frame callback.

### Sintoma

Uma mudança deixou de fazer commit da superfície do próprio veil a cada frame callback enquanto um vídeo é reproduzido, porque o `waylandsink` faz commit de suas próprias subsuperfícies (elas são dessincronizadas, `gstwlwindow.c` do GStreamer 1.28.7) e um sway headless mostrou o vídeo corretamente sem esses commits: a imagem se movia a 30 quadros por segundo, posicionada como antes, em `fill`, `fit` e `center`. No Hyprland, a mesma compilação deixou o vídeo "extremamente travado", e ele parecia "cortado com várias camadas": partes de quadros antigos umas sobre as outras. Isso não acontecia antes da mudança. O loop foi restaurado no mesmo dia, exatamente como era, e com ele o vídeo voltou a ser reproduzido com fluidez no Hyprland (verificado pelo mantenedor, 2026-10-16).

### O que se sabe

- O loop faz commit da superfície do veil (a pai das subsuperfícies do sink) uma vez por frame callback, isto é, uma vez por atualização de sua saída, sem novo buffer e sem dano.
- As subsuperfícies do sink são dessincronizadas: pelo protocolo, seus commits têm efeito imediato, sem um commit do pai.
- O que o Hyprland faz de diferente não foi examinado (seu código-fonte, ou `WAYLAND_DEBUG` e `HYPRLAND_TRACE` em sua sessão). Um palpite, não verificado: ele repinta apenas o dano da subsuperfície, no lugar ou no momento errados, a menos que o commit do pai o faça repintar a superfície inteira.

### Decisão (2026-10-16)

Manter o loop. Seu custo são despertares, não CPU: em um sway headless, de 160 a 175 despertares por segundo do loop de eventos do veil em `fit`, contra 75 a 85 sem ele, e nenhuma diferença mensurável na CPU, no veil nem no compositor (`docs/measurements.md`).

### Quando revisar

Quando a causa for conhecida, ou se uma mudança posterior alterar como os quadros chegam à tela. Substituir o `waylandsink` (2026-11-09, entrada 1) manteve tudo de que esta entrada depende: a subsuperfície do vídeo é dessincronizada e recebe commit do sink, como a do `waylandsink`, e o loop faz commit da superfície do veil como antes. Nenhum teste cobre isto: só o Hyprland mostrou o problema, e a renderização do Hyprland não é coberta por testes ([`testing.md`](https://github.com/ordreth/veil/blob/stable/docs/testing.md), "Not covered by any test"). Uma mudança no loop passa nos testes headless de qualquer forma, então ele fica como está até a causa ser conhecida; uma regressão apareceria no uso diário no Hyprland.

Não tentado: fazer commit na taxa de quadros do vídeo em vez de a cada frame callback (a alternativa considerada quando o loop foi medido). Esse trabalho foi encerrado em 2026-10-16 sem isso, por decisão do mantenedor, com sua meta (despertares próximos da taxa de quadros do vídeo) não atingida. Fica para depois da 1.0.

## 7. Um vídeo custa CPU: seus quadros são convertidos para RGB na CPU

**Situação:** aberto, aceito para a 1.0 (2026-10-17); resolvido para decodificadores VA-API cujos dmabufs o compositor aceita (2026-11-20, abaixo). **Afeta:** todo papel de parede em vídeo cujo decodificador entrega seus quadros em memória de sistema, o que na máquina de referência (NVIDIA) é todo decodificador. A meta de "menos de 1 % de CPU" para vídeos não é atingida; as imagens não são afetadas (0 %).

### Sintoma

Com o decodificador por hardware da NVIDIA, o daemon usa cerca de 18 % de um núcleo para um vídeo 1080p a 30 fps, 37 % a 60 fps e 135 % para 2160p a 60 fps (`docs/measurements.md`, "Where a video's CPU goes"). O custo segue os pixels por segundo, não o codec.

### Causa

A decodificação custa cerca de 1 % de um núcleo. O resto é o `videoconvert` no sink bin do veil transformando cada quadro NV12 em RGB. Ele precisa fazer isso: nenhum compositor daqui aceita NV12 em memória compartilhada (o `wl_shm` do sway headless e do Hyprland 0.56.2 aceita apenas formatos RGB). Um compositor só pode aceitar um quadro NV12 como está na forma de um dmabuf, e na NVIDIA nenhum dos decodificadores que o GStreamer tem entrega um. Os decodificadores `nvcodec` entregam memória de sistema, CUDA ou GL; os decodificadores VA-API do driver VA da NVIDIA entregam apenas memória de sistema.

### O que o veil faz a respeito

- Nenhum conversor próprio do `playbin` (`native-video`), então o do sink bin é o único, e ele deixa passar os quadros que o sink aceita como estão (`video/x-raw(ANY)`, dmabufs incluídos).
- A conversão roda em duas threads, de modo que 4K a 60 fps é exibido por inteiro (ele perdia de 15 a 25 % dos quadros com uma thread), por 1 a 4 pontos a mais de um núcleo em 1080p.
- O decodificador é registrado no log quando um vídeo inicia (`video decoder: nvh264dec (hardware)`), de modo que um relato diz qual caminho foi seguido.

### Opções consideradas

| Opção | O que dá | Por que não foi feita (ainda) |
|---|---|---|
| **A. Decodificadores VA-API com saída DMABuf** | Cópia zero: os quadros do decodificador vão ao compositor como estão. | Feito (2026-11-20, abaixo). Precisa de um driver VA-API da Intel ou da AMD; na NVIDIA, o driver VA não entrega dmabufs ao GStreamer. |
| **B. Converter na GPU** (`cudaconvertscale`, `glcolorconvert`, Vulkan) | A CPU apenas copia o quadro RGB. | O `cudaconvertscale` não está nesta compilação do `nvcodec`; o caminho GL não negociou aqui e traria um contexto GL para o daemon, com uma cadeia de elementos que só serve para a NVIDIA. Não levada adiante. Verificado de novo em 2026-11-20 com o GStreamer 1.28.8: o `nvcodec` registra `cudaconvert`, `cudascale` e `cudaconvertscale` só onde o compilador em tempo de execução da NVIDIA (NVRTC, do CUDA toolkit, não do driver) carrega, então a maioria dos sistemas também não os teria; `nvh264dec ! glcolorconvert ! gldownload` continuou sem negociar, sem display e com EGL surfaceless; não existe `vulkancolorconvert`. O que resta é a opção D, um contexto de GPU próprio do veil. |
| **C. Dumb buffers do DRM** (`waylandsink drm-device=...`) | NV12 copiado para um dmabuf, sem conversão. | O sway só aceita dmabufs NV12 com os modificadores tiled da NVIDIA, não como dumb buffers lineares: ele ainda convertia (21.5 %). |
| **D. Substituir o `waylandsink`** (entrada 1, opção E) | Um sink próprio do veil poderia receber quadros CUDA ou GL, exportá-los como dmabufs, e também eliminaria as entradas 1 e 2 e os despertares da entrada 6. | Grande, e depois da 1.0. |

### Desde 2026-11-09

O sink próprio do veil substituiu o `waylandsink` (entrada 1). A conversão é o mesmo código (`GstVideoConverter`, duas threads), agora dentro do sink, direto no buffer `wl_shm`, e ela pula o que `fill` e `center` recortam. O custo de um vídeo é o mesmo, dentro do ruído (`measurements.md`, "veil's own sink"). O caminho para um vídeo mais barato não mudou: dmabufs (VA-API em Intel e AMD, que o sink ainda não aceita) ou uma conversão na GPU.

### Desde 2026-11-20: quadros em dmabufs

O sink aceita os quadros de um decodificador em dmabufs (`video/x-raw(memory:DMABuf)`, `DMA_DRM`) quando o compositor aceita seu formato e modificador (`zwp_linux_dmabuf_v1`), e os exibe como estão: sem conversão, sem cópia, o recorte é o do viewport. Em uma GPU Intel (Raptor Lake, `vah264dec`) em um sway headless renderizando nessa GPU, H.264 em 1080p30 custa cerca de 2 % de um núcleo em vez de 45 %, e 4K a 60 fps é reproduzido por inteiro por cerca de 7 % em vez de perder um terço dos quadros a 175 % (`measurements.md`, "Frames in dmabufs").

O que resta:

- **NVIDIA:** sem mudança. Os decodificadores dela não entregam dmabufs (seria preciso a opção B ou D).
- **As cores exigem que o compositor saiba como estão codificadas.** O próprio compositor transforma o quadro NV12 em RGB, e é informado da codificação (BT.709, BT.601, BT.2020, a faixa) por meio do `wp_color_representation_v1`. Um que não pode ser informado usa BT.601 com a faixa limitada, o padrão da importação de dmabuf do EGL, e mostraria um vídeo BT.709 com cores erradas (medido: vermelho 231 em vez de 251, ciano com vermelho). O veil então só lhe entrega vídeos codificados como BT.601, e converte os outros na CPU, como antes. O sway 1.12 renderizando com `gles2` anuncia o protocolo sem nenhuma codificação; com `vulkan` ele anuncia BT.601, BT.709 e BT.2020 nas duas faixas. A maior parte dos vídeos HD é BT.709 (e o GStreamer toma um vídeo HD sem marcação por BT.709), então em um compositor que não pode ser informado a maioria dos vídeos continua sendo convertida.
- **O Hyprland até a 0.57 não aceita dmabufs YUV.** O renderizador dele deixa todo formato YUV (NV12, P010, ...) de fora dos formatos que lista (`Do not advertise YUV until the renderer supports it`, `src/render/OpenGL.cpp`), e ele não tem `wp_color_representation_v1`, então lá todo vídeo é convertido na CPU, inclusive os BT.601; o log diz isso no início (`compositor takes dmabufs in no YUV format`). Visto na 0.57.2 (2026-11-20). O branch principal do Hyprland (novembro de 2026) lista formatos YUV, amostrados como texturas externas, e tem `wp_color_representation_v1`: o lançamento depois da 0.57 deve aceitar os quadros do veil sem nenhuma mudança no veil. Ainda não testado.
- **Um decodificador não negocia de novo enquanto reproduz.** Quando o compositor recusa um dmabuf, ou não pode ser informado da codificação de um vídeo, o sink oferece só memória de sistema e o pipeline é iniciado mais uma vez (os decodificadores VA-API ignoram um pedido para negociar de novo); enquanto isso, o papel de parede anterior fica na tela.
- **Notebooks com duas GPUs:** o GStreamer dá aos decodificadores da NVIDIA um rank acima dos VA-API, então um notebook com as duas decodifica H.264, HEVC e AV1 na placa NVIDIA, em memória de sistema, mesmo quando o compositor roda na GPU Intel e aceitaria os dmabufs dela (o VP9 foi para o `vavp9dec` na máquina medida). `GST_PLUGIN_FEATURE_RANK=nvh264dec:0,nvh265dec:0,nvav1dec:0` faz o GStreamer escolher os decodificadores VA-API (`troubleshooting.md`). O veil não escolhe o decodificador sozinho.

### Decisão (2026-10-17)

Manter o `waylandsink` na 1.0, com o pipeline correto mais barato (`native-video`, duas threads de conversão), e registrar o custo como foi medido. A conversão não é obra do `waylandsink`: qualquer sink que mostre os quadros da NVIDIA por memória compartilhada precisa dela.

### Quando revisar

Quando o `nvcodec` ou o driver VA da NVIDIA puderem entregar dmabufs, quando uma conversão na GPU for retomada (opção B), quando o suporte do Hyprland ao `wp_color_representation_v1` for conhecido (ele decide se um vídeo BT.709 segue o caminho dmabuf ali), e quando um testador relatar um vídeo em hardware AMD.

## 8. Versões mais antigas do GStreamer: GIFs animados e quadros descartados no Ubuntu 24.04

**Situação:** aberto, documentado (2026-10-19). A montante, no GStreamer e no FFmpeg das distribuições; não no veil. **Afeta:** o tarball de lançamento (que usa o GStreamer do sistema) no Debian 12, no Ubuntu 22.04 e no Ubuntu 24.04. O pacote Nix traz o GStreamer 1.28 e não é afetado, nem o Debian 13, o Fedora 44 e o Arch.
**Última verificação:** 2026-10-19, com o tarball x86_64 da 0.1.0 em um contêiner de cada distribuição, contra um sway headless no host, à mão (não é um teste: [`testing.md`](https://github.com/ordreth/veil/blob/stable/docs/testing.md), "Checks to be turned into tests").

### Sintoma

| Distribuição | GStreamer, FFmpeg | GIF animado | Vídeos |
|---|---|---|---|
| Ubuntu 22.04 | 1.20.3, 4.4 | Mostra os primeiros quadros e depois fica parado (`veil status` diz `playing`; 4 ticks de CPU em 5 s) | são reproduzidos |
| Debian 12 | 1.22.0, 5.1 | Mostra os primeiros quadros e depois fica parado, e o daemon usa um núcleo inteiro da CPU enquanto ele for o papel de parede (499 ticks de CPU em 5 s, todos na thread `avdemux_gif0:sink` do GStreamer) | são reproduzidos |
| Ubuntu 24.04 | 1.24.2 (`gst-libav` 1.24.1), 6.1 | Nunca inicia: `veil set` falha com `the video did not show a frame within 10s`, e o papel de parede anterior permanece | os vídeos 1080p mostraram de metade a um quarto de seus quadros (`veil status`: H.264 a 30 fps 133 exibidos e 148 descartados, VP9 a 60 fps 51 e 167); um de 320x180 mostrou todos em `center`, quase todos em `fill` |
| Debian 13, Fedora 44, Arch | 1.26.2, 1.28.8 | é reproduzido | são reproduzidos |

### Causa

As falhas com GIF estão no demuxer FFmpeg do GStreamer (`avdemux_gif`), sem envolver o veil: `gst-launch-1.0 filesrc location=anim.gif ! decodebin ! fakesink sync=true` dá a todo quadro o timestamp 0 e os repete sem fim no Debian 12 (então o stream nunca chega ao fim e a thread do demuxer nunca descansa), e nunca faz o preroll no Ubuntu 24.04. O Ubuntu 22.04 só foi visto pelo veil. A mesma animação convertida para VP9 em WebM é reproduzida nos três (três capturas de tela diferentes em três).

Os quadros descartados no Ubuntu 24.04 são contados pelo seu `waylandsink` (o decodificador e a conversão usaram alguns por cento de um núcleo, então a CPU não é o limite). A causa não foi estabelecida. Não foi possível fazer um controle que exclua o veil: um simples `gst-launch-1.0 ... ! waylandsink` em uma janela própria descarta cerca de metade dos quadros do mesmo arquivo no sway headless também no Debian 13, onde o veil não descarta nenhum. Se acontece com um compositor em hardware real, não foi verificado.

### Opções

| Opção | Por que não (por enquanto) |
|---|---|
| **A.** Exigir o GStreamer 1.26 para o tarball | Deixa de fora o Debian 12 e as duas versões LTS do Ubuntu, onde imagens e vídeos funcionam. |
| **B.** Recusar GIFs animados quando o GStreamer em tempo de execução for anterior à 1.26 | Não se sabe qual dos dois, GStreamer ou FFmpeg, o bug acompanha, então uma verificação de versão seria um palpite. |
| **C.** Documentar, com a solução alternativa: converter o GIF em um vídeo (`troubleshooting.md`) | Escolhida. |

### Decisão (2026-10-19)

C. O README lista as distribuições em que o tarball foi executado e aponta para cá; `troubleshooting.md` diz como transformar um GIF em um WebM que é reproduzido em toda parte. Para o mantenedor confirmar ou mudar.

### Quando revisar

Quando um testador relatar o tarball em uma destas distribuições com um compositor em hardware real (especialmente o Ubuntu 24.04 e seus quadros descartados), ou quando mudarem as distribuições mais antigas a que o tarball se destina.

---

## 9. Cores desbotadas de decodificadores VA-API em memória de sistema

**Situação:** corrigido depois da 1.0.0-beta.1 (2026-11-20, veja "Corrigido" no fim desta entrada). **Afetava:** vídeos decodificados por um decodificador VA-API (Intel, AMD) cujos quadros o veil converte na CPU: todo vídeo assim na 1.0.0-beta.1, e depois os que não podem ir ao compositor em dmabufs (entrada 7), o que no Hyprland até a 0.57 é todo vídeo.
**Última verificação:** 2026-11-20, `vah264dec` (GStreamer 1.28.8) em uma GPU Intel, um sway headless renderizando com `pixman` (quadros convertidos na CPU).

### Sintoma

As cores ficam mais pálidas e mais escuras do que são: em clipes de barras de cores codificados como BT.709 e como BT.601, ambos em faixa limitada, o vermelho puro aparece como cerca de (237, 13, 15) em vez de (251, 0, 0) (a decodificação do ffmpeg), e o branco fica cinza. Os mesmos clipes em dmabufs, convertidos pelo compositor, ficam certos.

### Causa

As caps do decodificador em memória de sistema dizem `colorimetry=0:3:0:0`: a matriz (BT.709), mas uma faixa desconhecida. O conversor do GStreamer toma uma faixa desconhecida pela completa e não estica os 16 a 235 de um vídeo para 0 a 255. O caminho dmabuf do veil toma uma faixa desconhecida pela limitada, como o próprio GStreamer faz quando completa uma colorimetria.

### O que corrigiria

Tomar a faixa desconhecida de um stream YCbCr pela limitada antes de o conversor ser criado (`sink.rs`, `new_converter`). Deixado para uma mudança própria: ela muda as cores de todo vídeo que vem por esse caminho, o que deve ser verificado à parte. Se os decodificadores da NVIDIA informam a faixa não foi verificado.

### Corrigido (2026-11-20)

Antes de o conversor ser criado, o que a colorimetria do stream deixa em aberto é preenchido como o caminho dmabuf faz: a faixa limitada, e a matriz que o GStreamer supõe para a altura do quadro (`with_known_coding` em `sink.rs`). Uma faixa ou uma matriz que o stream informa é mantida. Nos mesmos clipes de barras de cores, convertidos na CPU, as cores agora são as do ffmpeg dentro de 3 (vermelho (251, 0, 0) para BT.709, (253, 0, 0) para BT.601), como em dmabufs. O custo de um vídeo é o mesmo: H.264 1080p30 com `vah264dec`, de 40.9 a 41.2 % de um núcleo contra 40.7 a 41.0 % antes, todos os quadros exibidos (quatro execuções alternadas de 30 s cada; a primeira execução da compilação antiga, 35.2 %, ficou fora de todas as outras). Se os decodificadores da NVIDIA informam a faixa continua não verificado; eles recebem o mesmo tratamento se não informarem.
