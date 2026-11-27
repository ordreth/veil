---
title: "Medições"
description: "Como o veil é medido, e todos os números até agora."
group: "Reference"
order: 7
source: "docs/measurements.md"
source_hash: "ff7dfdfb6c6687f0ca9f79428bfe9df89e1de2501a6ee9d6a2e4c82ccaaaa031"
---

Quanto o daemon custa enquanto exibe um papel de parede, medido com `scripts/bench/`. As metas de desempenho do README são verificadas contra estes números (menos de 1% de CPU para vídeo, 0% para imagens estáticas, nenhum crescimento); onde não são atingidas, este arquivo diz isso. Só contam os números dos scripts ([`testing.md`](https://github.com/ordreth/veil/blob/stable/docs/testing.md), "Numbers"); os poucos mais antigos medidos de outro jeito estão marcados como tais e mantidos como histórico.

## Como executar

Dentro do `nix develop .#headless` (sway, grim, jq, ffmpeg e os plugins do GStreamer), com uma compilação de lançamento:

```bash
cargo build --release
scripts/bench/clips.sh                                      # the reference clips, in target/bench/clips
scripts/bench/bench.sh target/bench/clips/h264-1080p30.mp4  # 5 s warm-up, then 60 s of samples
```

`bench.sh FILE` inicia um sway headless próprio (renderização pixman, uma saída de 1920x1080; `--size` a altera), executa `veil daemon --input FILE` nele, com diretórios próprios (seus papéis de parede salvos e seu arquivo de configuração não são lidos nem escritos), espera até o papel de parede ser exibido, espera `--warmup` segundos e então coleta amostras do daemon uma vez a cada `--interval` (1 s) durante `--duration` (60 s). Os resultados vão para `target/bench/<date>-<file>/` (`--out` para escolher): `samples.csv`, `report.txt` (também impresso), e os logs do daemon, do sway e dos elementos do GStreamer que o pipeline criou. `scripts/bench/summary.sh samples.csv` imprime de novo o resumo de uma execução.

Outras opções: `--mode` (o `--mode` do veil), `--renderer gles2` ou `--renderer vulkan` (o sway headless renderiza em uma GPU e aceita dmabufs, em vez do `pixman` na CPU) com `--render-device /dev/dri/renderD128` (o nó de renderização dessa GPU; senão o wlroots escolhe um), `--veil PATH` (outro binário, por exemplo `./result/bin/veil` do `nix build`, para comparar compilações) e `--outputs N` (N saídas headless lado a lado, cada uma reproduzindo o arquivo em um pipeline próprio; as contagens de quadros são somadas), e `--session`, que roda o daemon na sessão Wayland em que o script é iniciado, em vez de em um sway headless (pare antes o seu próprio daemon veil: um segundo se recusa a iniciar; `--output NAME` o limita a um monitor). As variáveis de ambiente chegam ao daemon: `GST_PLUGIN_FEATURE_RANK=nvh264dec:0` tira um decodificador do caminho, `MALLOC_ARENA_MAX=1` muda o alocador (o `target/release/veil` tem o padrão da glibc, a menos que ela seja definida; o relatório a nomeia).

### As colunas de `samples.csv`

| Coluna | O que é |
|---|---|
| `t_s` | segundos desde a primeira amostra |
| `cpu_pct` | tempo de CPU do daemon (todas as suas threads, usuário e sistema, de `/proc/<pid>/stat`) ao longo do intervalo, em porcentagem de um núcleo |
| `rss_kib` | memória residente (`VmRSS`) |
| `threads`, `fds` | entradas de `/proc/<pid>/task` e `/proc/<pid>/fd` |
| `sink_rendered`, `sink_dropped` | as contagens de quadros do próprio sink de vídeo, somadas entre as saídas, de `veil status --json`. Elas recomeçam em 0 a cada repetição do vídeo, então o resumo calcula a taxa de quadros sobre os intervalos sem repetição. Um quadro que o sink nunca recebeu (descartado antes no pipeline, por um decodificador que fica para trás) não está em nenhuma das colunas. |
| `gpu_pct`, `gpu_decoder_pct` | com `nvidia-smi`: a utilização da GPU inteira e de seu decodificador de vídeo. **Do sistema todo**: todo processo conta, inclusive o desktop em que o script roda. Vazio sem `nvidia-smi`. |
| `wakeups_per_s` | trocas de contexto da thread do loop de eventos do daemon (a thread principal, `/proc/<pid>/task/<pid>/status`) por segundo ao longo do intervalo: quantas vezes ela foi despertada. O `veil status --json` que o script envia uma vez por amostra também conta (cerca de 2 por segundo; uma imagem estática não mostra mais nada). É a última coluna, acrescentada depois das outras, de modo que arquivos de execuções antigas ainda são lidos. |

O `report.txt` acrescenta o clipe (tamanho, taxa, codec pelo `ffprobe`), o decodificador que o GStreamer criou (de `GST_DEBUG=GST_ELEMENT_FACTORY:4`; se ele tentou vários, todos são nomeados), a compilação do veil (versão e commit), o compositor, a CPU, a GPU e o kernel.

Observações: os quadros descartados vêm dos contadores do próprio sink (os mesmos do `veil status`) e não das mensagens de QoS do GStreamer; a coluna de GPU suporta apenas NVIDIA (`intel_gpu_top` e `radeontop` precisam de hardware que a máquina de referência não tem, e não são cobertos).

## Máquina de referência e clipes

| | |
|---|---|
| CPU | Intel Core i5-11400F (6 núcleos, 12 threads, 2.6 GHz), sem GPU integrada |
| GPU | NVIDIA GeForce RTX 3060 Ti, driver do NixOS; sem driver VA-API, então o plugin `va` do GStreamer não oferece nada e os decodificadores `nvcodec` são usados (`nvh264dec` e os outros têm rank acima do `avdec_h264`) |
| Memória | 32 GB |
| Sistema | NixOS, Linux 7.2.0, GStreamer 1.28.7 (o conjunto de plugins do pacote), sway 1.12 headless (pixman), saída 1920x1080 na escala 1 |

Os clipes vêm de `scripts/bench/clips.sh`: o `testsrc2` do ffmpeg (todo quadro é diferente), de 10 segundos cada, de modo que uma execução de 60 s repete seis vezes.

| Arquivo | Codec | Tamanho, taxa |
|---|---|---|
| `h264-1080p30.mp4` | H.264 High (libx264, CRF 23) | 1920x1080, 30 fps |
| `h264-1080p60.mp4` | H.264 High | 1920x1080, 60 fps |
| `h264-2160p60.mp4` | H.264 High | 3840x2160, 60 fps |
| `hevc-2160p60.mp4` | HEVC Main (libx265, CRF 26) | 3840x2160, 60 fps |
| `vp9-1080p60.webm` | VP9 Profile 0 (libvpx, 4 Mbit/s) | 1920x1080, 60 fps |
| `av1-1080p60.mkv` | AV1 Main (SVT-AV1, CRF 35) | 1920x1080, 60 fps |
| `still-1080p.png` | PNG | 1920x1080, um quadro |

## Resultados (2026-10-16, sway headless)

Cada linha é uma execução de 60 amostras após um aquecimento de 5 s, `--mode fill` salvo indicação em contrário, compilada a partir da árvore de trabalho daquele dia. A CPU é a média (a mediana ficou a até 0.6 ponto dela em todas as execuções); a memória é o conjunto residente no fim da execução.

| Papel de parede | Decodificador | CPU (um núcleo) | Quadros exibidos por s | Descartados pelo sink | RSS | Threads | Descritores |
|---|---|---|---|---|---|---|---|
| `still-1080p.png` | nenhum | **0.00 %** (0 ticks em 60 s) | nenhum | nenhum | 19.7 MiB | 1 | 15 |
| `h264-1080p30.mp4` | `nvh264dec` | 17.2 % | 30.0 | 0 | 202.7 MiB | 14 a 15 | 65 |
| `h264-1080p30.mp4`, `--mode fit` | `nvh264dec` | 17.0 % | 30.1 | 0 | 202.8 MiB | 14 a 15 | 65 |
| `h264-1080p60.mp4` | `nvh264dec` | 34.1 % | 60.1 | 0 | 201.3 MiB | 14 a 15 | 65 |
| `vp9-1080p60.webm` | `nvvp9dec` | 34.0 % | 60.1 | 0 | 248.9 MiB | 14 a 15 | 65 |
| `av1-1080p60.mkv` | `nvav1dec` | 33.9 % | 60.1 | 0 | 201.5 MiB | 14 a 15 | 65 |
| `h264-2160p60.mp4` | `nvh264dec` | 108.0 % | **51.1** | 0 | 296.2 MiB | 14 a 15 | 65 |
| `hevc-2160p60.mp4` | `nvh265dec` | 108.0 % | **51.3** | 0 | 295.4 MiB | 14 a 15 | 65 |
| `h264-1080p30.mp4`, software | `avdec_h264` | 22.1 % | 30.0 | 0 | 161.7 MiB | 21 a 22 | 27 |
| `h264-2160p60.mp4`, software | `avdec_h264` | 153.3 % | 60.0 | 0 | 456.5 MiB | 21 a 22 | 27 |

"Software" é `GST_PLUGIN_FEATURE_RANK=nvh264dec:0,nvh265dec:0,nvav1dec:0`. As colunas de GPU ficam de fora da tabela: elas são do sistema todo, e o desktop de onde as execuções foram iniciadas usou de 1 a 20 % da GPU por conta própria (a imagem estática, que não usa GPU nenhuma, marcou 8.9 % em média). A utilização do decodificador foi de 4 % para H.264 1080p30, 10 % para 1080p60 e 35 % para H.264 2160p60 (15 % para HEVC), e 0 com a decodificação por software.

### O que os números dizem

- **Uma imagem estática não custa nada**: nenhum tempo de CPU em 60 s, uma thread, 15 descritores, 20 MiB. A meta de 0.0 % de CPU é atingida aqui (sway headless; ainda não medida em uma sessão real).
- **Um vídeo está longe da meta de 1 %.** Com o decodificador por hardware, o daemon ainda usa cerca de 17 % de um núcleo para 1080p a 30 fps, e o custo cresce com o número de pixels por segundo (34 % a 60 fps, 108 % para quatro vezes os pixels), não com o codec: H.264, VP9 e AV1 em 1080p60 custam o mesmo. Então não é a decodificação que custa: os quadros são copiados na CPU depois que a GPU os decodificou, o que é examinado em "Para onde vai a CPU de um vídeo", abaixo. Nesta configuração alguma cópia é inevitável: o sway headless com pixman não oferece `zwp_linux_dmabuf_v1`, então o `waylandsink` tem de pôr todo quadro em memória compartilhada. Uma sessão real (`--session`) é a medição que diz quanto disso é do pipeline do veil. *Essa investigação descobriu que é a conversão de cor do `videoconvert`, que acontece em uma sessão real na NVIDIA também ("Para onde vai a CPU de um vídeo", abaixo).*
- **O caminho por hardware não acompanha 4K a 60 fps aqui**: 51 quadros por segundo chegam ao sink e o daemon usa mais de um núcleo. O sink não conta nada como descartado, então os quadros que faltam se perdem antes dele (por uma parte do pipeline que fica para trás; qual, não foi estabelecido). A decodificação por software mostra todos os 60, a 153 %. Uma causa provável é a mesma cópia, feita em uma única thread de streaming; "Para onde vai a CPU de um vídeo" confirma isso. *Confirmado ali: com duas threads de conversão, todos os 60 quadros são exibidos.*
- **O pipeline tem mais elementos que o sink bin do veil**: além de `videoconvert ! videocrop ! waylandsink`, o `playbin` cria um `videoconvert`, um `videoscale` e um `videobalance` próprios (vistos no log de elementos de toda execução de vídeo). Onde eles ficam e se copiam quadros foi examinado em "Para onde vai a CPU de um vídeo". *Removidos desde então (`native-video`).*
- **Fill ou fit não faz diferença para a CPU** (17.2 % contra 17.0 %), embora a superfície do próprio veil receba frame callbacks em `fit` e nenhum em `fill`: o loop de commits não é onde o tempo vai (medido de novo em "O loop de frame callback", abaixo).
- **A memória e os descritores ficam estáveis enquanto um vídeo se repete** (seis repetições por execução): a inclinação do RSS ficou entre +33 e +144 KiB por minuto com o decodificador por hardware, dentro do ruído de uma execução de 60 s, e as threads e os descritores não mudaram. Com a decodificação por software, o RSS cresceu de 6 a 12 MiB durante a execução, o que pode ser o enchimento dos pools do decodificador depois do aquecimento; uma execução de 60 s não distingue isso de um vazamento. A execução longa está em "Testes de longa duração e o alocador", abaixo.
- **O sink não perde nenhum quadro em uma repetição**: 30.0 e 60.1 quadros por segundo nos intervalos sem repetição. A parada do problema conhecido 5 (a imagem ficando parada a cada repetição em `fill`) não é, portanto, visível nas contagens do sink, como a entrada 5 já diz; ela precisa ser medida na tela.

## O loop de frame callback (2026-10-16, sway headless)

O daemon faz commit de sua superfície a cada frame callback enquanto um vídeo é reproduzido, para que o compositor apresente a subsuperfície do vídeo. As subsuperfícies do `waylandsink` são dessincronizadas e recebem commit do próprio sink, então tentou-se sem esses commits e mediram-se os dois casos. **A mudança foi desfeita no mesmo dia**: no Hyprland o vídeo travava e mostrava partes de quadros antigos sem os commits (`known-issues.md`, entrada 6). Os números abaixo são o que o loop custa; "depois" é a compilação sem ele, que não é o que o veil faz. Medido com `bench.sh --size 2560x1080 --duration 30` (para que o `fit` deixe barras e a superfície do veil fique visível, o caso em que o loop rodava), `h264-1080p30.mp4`, `nvh264dec`, compilações de lançamento da árvore de trabalho antes e depois da mudança:

| Modo | Compilação | Despertares do loop de eventos por s | CPU (um núcleo) | Quadros exibidos por s |
|---|---|---|---|---|
| `fit` | antes | 158.4 | 18.3 % | 30.0 |
| `fit` | depois | **85.4** | 18.2 % | 30.1 |
| `fill` | antes | 88.7 | 18.0 % | 30.0 |
| `fill` | depois | 84.8 | 17.8 % | 30.0 |
| `still-1080p.png` | depois | 1.8 | 0.00 % | nenhum |
| `h264-1080p60.mp4`, `fill`, 1920x1080 | depois | 200.9 | (não comparado) | 60.1 |

Medido sem os scripts, mantido como histórico, na mesma configuração (um clipe VP8 de 1280x1080 em uma saída de 1920x1080, de 10 a 20 s por execução, duas rodadas alternando as compilações): os frame callbacks na superfície do veil passaram de 58 por segundo para nenhum em `fit` e `center` (nenhum em `fill` antes também); a CPU da thread do loop de eventos ficou na resolução do contador (0.15 a 0.4 %), e a do sway também (7.5 % em `fit`, 0.3 % em `fill`, sem mudança).

O que isso diz:

- **O loop custa despertares, não CPU.** Um despertar por atualização da tela e por saída, cerca de 70 por segundo aqui, e nada mensurável em CPU, no veil nem no sway. Ele fica, porque o Hyprland precisa dele.
- **O loop de eventos ainda desperta cerca de três vezes por quadro de vídeo** (85 a 30 fps, 201 a 60 fps), com ou sem o loop: são os eventos do próprio `waylandsink` (a resposta à sua requisição de sincronização, seu frame callback e a liberação do buffer anterior) na conexão Wayland que o veil compartilha com ele, que despertam também o poll do veil nessa conexão. Uma imagem estática não o desperta de forma alguma (os 1.8 por segundo são o `veil status` do próprio script). A parte do sink só some com a substituição do `waylandsink` (depois da 1.0), porque a subsuperfície do sink precisa ser criada na conexão do veil.

## Testes de longa duração e o alocador (2026-10-17, sway headless)

O `scripts/bench/soak.sh` roda o daemon por muito tempo na mesma configuração do `bench.sh` e falha quando sua memória, suas threads ou seus descritores de arquivo sobem:

```bash
scripts/bench/soak.sh loop     # a 1 s H.264 clip, so it loops once a second; 10 min, sampled every second
scripts/bench/soak.sh switch   # 500 `veil set`: video A, image B, video B, image A, ...; sampled after each
```

As mídias são feitas uma vez, em `target/bench/soak-media` (`--clip FILE` para outro clipe em repetição): dois PNGs de 1920x1080, o vídeo A H.264 1280x960 e o vídeo B VP9 2560x1080, de 3 s cada a 30 fps, como nas medições de memória de `known-issues.md`, entrada 3. Cada execução escreve `samples.csv`, `report.txt` (também impresso) e `graph.svg` (cada coluna contra o tempo ou a troca, a janela estável marcada, suas retas de mínimos quadrados tracejadas) em `target/bench/soak-<test>-<date>/`. O daemon recebe `MALLOC_ARENA_MAX=1`, como o pacote Nix e a unidade do systemd o dão (`--arenas N|default` para mudar; `default` deixa por conta da glibc, que permite 8 arenas por thread de CPU); `--mmap-threshold BYTES` define `MALLOC_MMAP_THRESHOLD_`.

**O que é testado.** O heap (`RssAnon`: o que o alocador retém, sem as bibliotecas e sem os buffers `wl_shm` da superfície) ao longo da janela estável, que deixa de fora o primeiro quinto da execução (`--settle`). No `switch`, só contam as amostras com uma imagem na tela: nenhum pipeline existe então, de modo que o que sobra é o que o alocador reteve. O teste é a subida dos limites da janela (`scripts/bench/trend.sh`): a menor amostra do último quarto da janela contra a menor do primeiro quarto, o mesmo para a maior, e dos dois resultados o mais próximo de zero. Uma reta de mínimos quadrados não funciona aqui: o heap salta entre níveis separados por alguns MiB (após uma troca, conforme o que o alocador por acaso reteve, 9 MiB a mais depois da imagem B do que depois da imagem A com uma arena; enquanto um decodificador por software reproduz, conforme seus buffers estejam ocupados no momento da amostra, cerca de 20 MiB), o que inclina a reta em ±20 KiB por troca ao longo de uma janela de 100 trocas. A memória retida de vez eleva os dois limites; esse ruído move um só. Limites: 8 KiB por minuto para `loop` (11 MiB por dia), 2 KiB por troca para `switch` (1 MiB em 500 trocas; `--max-rise`). As threads e os descritores podem subir 0.05 por minuto e 0.01 por troca (mínimos quadrados; um vazamento era de um de cada por troca antes de o vazamento da thread do bus ser corrigido). O `loop` também falha quando o vídeo mostra menos de 90 % de sua taxa de quadros ou não se repete, e o `switch` quando um `veil set` falha. As repetições e os quadros vêm da linha `debug` da thread do bus a cada repetição: os contadores do próprio sink recomeçam a cada repetição, então um clipe de 1 s não pode ser contado coletando-os uma vez por segundo.

O CI roda uma versão curta dos dois (job `soak`: `loop --duration 180`, `switch --switches 200`, decodificação por software nos runners do GitHub) e guarda os relatórios e gráficos como artefato. Ensaiado aqui com decodificação por software: os dois passam (os limites do heap não subiram na repetição, +0.23 KiB por troca).

### Um vídeo em repetição

| Clipe, decodificador | `MALLOC_ARENA_MAX` | Em 10 minutos | Subida do heap | Threads, descritores | Quadros por s, CPU |
|---|---|---|---|---|---|
| H.264 1080p30 de 1 s, `nvh264dec` | 2 | RSS 202.1 MiB, o mesmo até o KiB do 15º segundo ao fim | 0 | 15, 64 | 29.7, 16.8 % |
| `h264-2160p60.mp4`, `avdec_h264` | 2 | heap de 370 a 432 MiB; seu topo em 432,428 KiB a partir do terceiro minuto | nenhuma | 22, 26 | 59.6, 152 % |
| H.264 1080p30 de 1 s, `avdec_h264` | 2 | heap de 102 ou 122 MiB; o topo subiu aos poucos de 122,380 para 122,876 KiB nos primeiros 8 minutos | +32 KiB/min a partir de 120 s (falha), 0 a partir de 300 s | 22, 26 a 27 | 29.6, 25.4 % |

608 repetições do clipe decodificado por hardware não mudaram nada, então a repetição em si (um seek com flush no mesmo pipeline) não retém nada. O clipe de 1 s do decodificador por software reteve cerca de 500 KiB a mais nas primeiras 500 repetições e depois nada mais nos últimos cinco minutos, o que parece o alocador se estabilizando e não um vazamento; se ele permanece estável por horas fica a cargo do teste de longa duração de 24 horas do checklist de lançamento ([`roadmap.md`](/apps/veil/docs/roadmap)). Todas as linhas rodaram com 2 arenas, antes de o número distribuído passar a ser 1; o ensaio no CI com 1 e decodificação por software passou.

### Trocas de papel de parede

500 trocas cada (250 vídeos), `nvh264dec` e `nvvp9dec`, cerca de 77 s por execução. "Imagem depois" é a amostra depois da imagem que substituiu um vídeo; "subida" é o teste acima sobre as trocas de 100 a 500.

| `MALLOC_ARENA_MAX` | Heap após 1 vídeo | Heap após 250 vídeos (RSS) | Subida do heap | Threads, descritores com uma imagem | `veil set` para uma imagem / um vídeo | CPU por troca |
|---|---|---|---|---|---|---|
| 1 | 42.5 MiB | **42.2 MiB** (117.1 MiB) | 0.00 KiB/troca | 7, 29 | 110 / 176 ms | 0.15 s |
| 2 | 41.2 MiB | 73.0 MiB (147.8 MiB), estável a partir da décima troca | 0.00 KiB/troca | 7, 29 | 111 / 177 ms | 0.15 s |
| padrão da glibc (96 arenas aqui) | 41.4 MiB | 129.1 MiB (203.8 MiB), ainda subindo | **+6.25 KiB/troca** (falharia) | 7, 29 | 112 / 176 ms | 0.15 s |
| padrão, `MALLOC_MMAP_THRESHOLD_=131072` (opção F) | 22.4 MiB | 107.9 MiB (182.7 MiB), ainda subindo | +0.86 KiB/troca | 7, 29 | 116 / 181 ms | 0.16 s |

As threads e os descritores foram os mesmos depois de toda imagem de toda execução, e 20 s depois do último vídeo o daemon tinha caído para 3 threads (a thread principal, o `pool-spawner` da GLib e uma thread da biblioteca da NVIDIA). Nenhum `veil set` falhou.

### O que as configurações do alocador custam à reprodução

`bench.sh`, 60 s por execução, uma saída e duas (`--outputs 2`, o mesmo arquivo nas duas, de modo que dois pipelines compartilham o alocador). "Software" é `GST_PLUGIN_FEATURE_RANK=nvh264dec:0`.

| Clipe, decodificador | Saídas | CPU: padrão / 2 / 1 | Quadros por s: padrão / 2 / 1 |
|---|---|---|---|
| `h264-1080p60.mp4`, `nvh264dec` | 1 | 33.9 / 33.5 / 33.6 % | 59.7 / 59.6 / 59.6 |
| `h264-2160p60.mp4`, `nvh264dec` | 1 | 108.1 / 108.0 / 107.9 % | 51.3 / 51.2 / 51.5 |
| `h264-2160p60.mp4`, software | 1 | 150.4 / 152.0 / 151.9 % | 60.2 / 60.0 / 59.9 |
| `h264-1080p60.mp4`, `nvh264dec` | 2 | 73.3 / 73.3 / 72.8 % | 119.9 / 119.8 / 120.0 |
| `h264-2160p60.mp4`, software | 2 | 363.5 / 361.9 / 357.6 % | 120.4 / 120.0 / 120.2 |

Nenhuma diferença além do ruído de uma execução para outra, também com dois vídeos 4K decodificados por software (umas 40 threads de streaming compartilhando uma arena). O `veil set` levou o mesmo tempo com cada configuração (tabela acima).

### O que os números dizem

- **As threads e os descritores de arquivo não vazam**, ao longo de 500 trocas e de 600 repetições.
- **Um vídeo em repetição não retém nada.** Com o decodificador por hardware, a memória residente não mudou nem uma página em dez minutos de repetições.
- **A memória retida depois das trocas é do alocador, e uma arena não retém nada dela.** Com o padrão da glibc, o heap após 250 vídeos é três vezes o que é após um, e ainda sobe; com 2 arenas ele sobe por dez trocas e depois se mantém; com 1 ele não sobe de forma alguma. A reprodução custa o mesmo com cada uma. Por isso o veil é distribuído com `MALLOC_ARENA_MAX=1` (era 2, escolhido antes de isto ser medido), e o `malloc_trim` (opção B de `known-issues.md`, entrada 3) não é necessário.
- **Um limiar de mmap fixo (opção F) não basta** sozinho: ele retém menos que o padrão no começo, sobe mesmo assim e custa um pouco de tempo por troca.
- A decodificação por software precisa de mais heap que o decodificador por hardware (72 contra 35 MiB com um vídeo sendo reproduzido, uma arena) e troca mais depressa aqui (53 contra 177 ms por vídeo; a diferença é a preparação do decodificador da NVIDIA).

## Para onde vai a CPU de um vídeo (2026-10-17)

Os primeiros resultados encontraram um custo que segue os pixels por segundo e não o codec. O pipeline foi então desmontado para descobrir por quê. Primeiro com `gst-launch-1.0` no mesmo sway headless (`h264-1080p30.mp4`, `nvh264dec`, 6 s por execução), depois com o próprio daemon (`bench.sh`, 30 s por execução).

### As peças, uma de cada vez

| Pipeline após `nvh264dec` | Compositor | CPU (um núcleo) |
|---|---|---|
| `fakesink sync=true` (só decodificação, quadros baixados para a memória do sistema) | nenhum | 0.8 a 1.0 % |
| `videoconvert ! video/x-raw,format=BGRx ! fakesink` (decodificação e conversão, sem compositor) | nenhum | 17.7 % |
| `videoconvert ! waylandsink` | sway, pixman | 18.8 % |
| `waylandsink` sem `videoconvert` | sway, pixman | não negociado |
| `videoconvert ! waylandsink` | sway, `gles2` na GPU NVIDIA (oferece `zwp_linux_dmabuf_v1`) | 18.3 % |
| `waylandsink` sem `videoconvert` | sway, `gles2` | não negociado |
| `videoconvert ! videocrop ! waylandsink drm-device=/dev/dri/card1` (NV12 em dumb buffers do DRM) | sway, `gles2` | 21.5 % (ainda converte: o sway só aceita dmabufs NV12 com os modificadores tiled da NVIDIA) |
| `playbin` como o veil o montava antes desta investigação (`flags=video`) | sway, pixman | 19.8 % |
| `playbin` com `flags=video+native-video` | sway, pixman | 18.5 % |

O que isso diz:

- **Decodificar não custa nada; converter custa tudo.** O NVDEC decodifica 1080p30 por cerca de 1 % de um núcleo. Os outros 17 % são o `videoconvert` transformando cada quadro NV12 em RGB em uma única thread da CPU. Escrever o resultado no buffer de memória compartilhada custa quase nada.
- **A conversão não pode ser evitada com este hardware.** O sink não aceita NV12 em memória compartilhada: o `wl_shm` do sway headless aceita apenas formatos RGB, e o do Hyprland também (0.56.2, uma sessão real, lido com `wayland-info`: `AR24`, `XR24`, `AB24`, `XB24` e suas formas de 10 bits; NV12 apenas como dmabuf). Um quadro NV12 só chega a um compositor sem conversão como um **dmabuf**, e nenhum dos decodificadores daqui produz um. Os decodificadores `nvcodec` entregam memória de sistema, CUDA ou GL. Os decodificadores VA-API existem por meio do driver VA da NVIDIA (`nvidia_drv_video.so`, com `GST_VA_ALL_DRIVERS=1 LIBVA_DRIVER_NAME=nvidia NVD_BACKEND=direct`: `vah264dec`, `vah265dec`, `vavp9dec`, `vaav1dec`, ...), mas também entregam apenas memória de sistema (suas caps de origem não têm `memory:DMABuf`). O `cudaconvertscale`, que converteria na GPU, não está nesta compilação do `nvcodec`. Uma conversão em GL (`glcolorconvert ! gldownload`) não negociou aqui e não foi levada adiante.
- **Os números do headless valem para uma sessão real na NVIDIA.** A conversão acontece no processo do veil seja qual for o compositor, porque nenhum compositor aceita NV12 em memória compartilhada. O que uma sessão real acrescenta ou economiza do lado do compositor não está nestes números.
- **O caminho que seria sem cópia está aberto em outras GPUs, mas não verificado.** Com um driver VA-API da Intel ou da AMD, o `vah264dec` entrega quadros DMABuf. O `videoconvert` e o `videocrop` aceitam `video/x-raw(ANY)`, `DMA_DRM` incluído, e passam esses quadros adiante intactos, e o `waylandsink` aceita dmabufs. Nenhum hardware assim aqui.
- **O `playbin` acrescentou um `videoconvert`, um `videoscale`, um `videobalance` e um `identity` próprios** na frente do sink bin do veil (vistos no log de elementos das primeiras execuções). Ele faz isso a menos que `native-video` esteja entre suas flags. Eles custam cerca de 1 ponto aqui, porque só um dos dois conversores converte. Agora eles não existem mais.

### O daemon antes e depois (sway headless, pixman)

`bench.sh --duration 30`, compilações de lançamento da árvore de trabalho. "Antes" é a compilação anterior à mudança; "native-video" acrescenta a flag; "2 threads" dá também ao `videoconvert` do sink bin `n-threads=2`, que é o que o veil faz agora. Quando uma célula teve duas execuções, as duas são dadas.

| Clipe, decodificador | Antes | native-video | native-video, 2 threads (distribuído) |
|---|---|---|---|
| `h264-1080p30.mp4`, `nvh264dec` | 19.3 %, 30.0 fps | 18.6 / 17.6 %, 30.0 fps | 20.0 / 18.7 %, 30.0 fps |
| `h264-1080p60.mp4`, `nvh264dec` | (primeira execução: 34.1 %, 60.1 fps) | 34.8 %, 60.0 fps | 38.9 / 36.2 %, 60.0 fps |
| `vp9-1080p60.webm`, `nvvp9dec` | (primeira execução: 34.0 %, 60.1 fps) | | 38.5 %, 60.1 fps |
| `h264-2160p60.mp4`, `nvh264dec` | 108.8 %, **44.8 fps** (primeira execução: 51.1) | 109.0 %, **50.7 fps** | 134.7 %, **59.9 fps** |
| `h264-2160p60.mp4`, `avdec_h264` | (teste de longa duração: 150 a 152 %, 60 fps) | | 168.3 %, 59.9 fps |

O que isso diz:

- **4K a 60 fps é reproduzido por inteiro agora.** Com uma thread de conversão, o caminho por hardware perdia de 15 a 25 % dos quadros: essa thread também espera o download da GPU, e mais de um núcleo de trabalho não cabe em uma thread. Duas threads mostram todos os 60 quadros por segundo, a 135 % de um núcleo.
- **A segunda thread custa um pouco em todos os outros casos:** de 1 a 4 pontos em 1080p, e 15 pontos para 4K decodificado por software, que também acompanhava com uma thread. Um vídeo 4K fluido foi escolhido em vez desses pontos. Uma configuração que escolhesse o número de threads pelo tamanho do quadro não foi tentada: é a escolha de decodificador do GStreamer, e não o tamanho do quadro, que decide se uma thread basta.
- **A meta de 1 % está fora de alcance com estes decodificadores e o `waylandsink`.** O que um vídeo custa na máquina de referência: cerca de 18 % de um núcleo em 1080p30, 37 % em 1080p60, 135 % em 2160p60. É tudo conversão, então um caminho mais barato precisa dos quadros do decodificador como dmabufs (VA-API em Intel ou AMD, que o `waylandsink` aceita como estão), ou de uma conversão na GPU (CUDA, GL ou Vulkan), que o `waylandsink` não consegue receber do `nvcodec`. Um sink próprio do veil poderia recebê-los (opção E de `known-issues.md`, entrada 1), depois da 1.0.
- O daemon registra no log o decodificador que usa quando um vídeo inicia: `video decoder: nvh264dec (hardware)`.

## Os números do README (2026-10-19, sway headless)

A tabela da seção "Performance" do README, medida de novo na compilação daquele dia (o caminho de vídeo como a investigação acima o deixou): `bench.sh` com os padrões (60 s após um aquecimento de 5 s, `fill`, uma saída de 1920x1080), `MALLOC_ARENA_MAX=1` como o pacote e a unidade iniciam o daemon, compilação de lançamento da árvore de trabalho. "Software" é `GST_PLUGIN_FEATURE_RANK=nvh264dec:0`.

| Papel de parede | Decodificador | CPU (um núcleo) | Quadros exibidos por s | Descartados pelo sink | RSS | Threads | Descritores |
|---|---|---|---|---|---|---|---|
| `still-1080p.png` | nenhum | 0.02 % (um tick em 64 s; a mediana 0) | nenhum | nenhum | 19.8 MiB | 1 | 15 |
| `h264-1080p30.mp4` | `nvh264dec` | 19.5 % | 30.0 | 0 | 203.4 MiB | 15 a 16 | 65 |
| `h264-1080p60.mp4` | `nvh264dec` | 39.7 % | 59.8 | 0 | 203.3 MiB | 15 a 16 | 65 |
| `vp9-1080p60.webm` | `nvvp9dec` | 37.9 % | 59.9 | 0 | 250.7 MiB | 15 a 16 | 65 |
| `h264-2160p60.mp4` | `nvh264dec` | 136.3 % | 60.2 | 0 | 297.1 MiB | 15 a 16 | 65 |
| `h264-1080p30.mp4`, software | `avdec_h264` | 24.5 % | 30.0 | 0 | 160.7 MiB | 22 a 23 | 27 |
| `h264-2160p60.mp4`, software | `avdec_h264` | 173.6 % | 60.1 | 0 | 458.4 MiB | 22 a 23 | 27 |

Eles concordam com a tabela acima (coluna distribuída: 18.7 a 20.0 %, 36.2 a 38.9 %, 38.5 %, 134.7 %, 168.3 %). A decodificação por software em 1080p30 custa mais que os 22.1 % dos primeiros resultados, que foram medidos com uma thread de conversão, antes da segunda. Como nos primeiros resultados, a memória residente com decodificação por software cresceu durante os 60 s (em 4 e 10 MiB por minuto), o que os testes de longa duração de dez minutos acima mostram se estabilizando.

**Uma primeira sessão no mesmo dia foi descartada.** O navegador do desktop usou de 9 a 11 das 12 threads de CPU durante ela (carga 12.8, encontrada com `top` depois das execuções). Suas execuções de 1080p concordaram com a tabela acima (18.2, 35.5 e 38.6 %), mas as execuções que precisam de vários núcleos não: 4K por hardware a 153.6 %, 1080p por software a 27.5 %, 4K por software a 221.4 %. A tabela é de uma segunda sessão com a máquina 92 % ociosa (o desktop rodando, como nas execuções anteriores). Uma execução do bench não diz nada sobre a carga da máquina; confira o `uptime` antes e depois de uma execução.

## O sink próprio do veil (2026-11-09, sway headless)

O `waylandsink` foi substituído por um sink próprio do veil (`known-issues.md`, entrada 1). As duas compilações foram medidas na mesma tarde, alternadas: "antes" é o commit `1c5d63b` (1.0.0-beta.1 e correções), "depois" a árvore de trabalho com o novo sink. `bench.sh --duration 30`, `fill`, uma saída de 1920x1080, `MALLOC_ARENA_MAX=1`. **Outra máquina que não a de referência acima**: Intel Core i7-13620H (16 threads), uma GPU NVIDIA GeForce RTX 4050 Laptop (os decodificadores `nvcodec`) e uma GPU Intel (VA-API; "VA-API" na tabela é `GST_PLUGIN_FEATURE_RANK=nvh264dec:0`, que escolheu o `vah264dec`, não um decodificador por software). Carga média de cerca de 2 durante as execuções.

| Clipe, decodificador | CPU antes / depois | Quadros por s antes / depois | Despertares por s antes / depois | RSS antes / depois |
|---|---|---|---|---|
| `h264-1080p30.mp4`, `nvh264dec` | 40.9 / 41.7 % | 29.9 / 30.0 | 85.2 / 33.8 | 157.5 / 183.4 MiB |
| `h264-1080p60.mp4`, `nvh264dec` | 81.8 / 78.5 % | 60.0 / 59.7 | 161.4 / 64.7 | 157.2 / 183.5 MiB |
| `h264-2160p60.mp4`, `nvh264dec` | 204.1 / 213.4 % | 60.1 / 59.9 | 159.9 / 81.3 | 250.8 / 357.1 MiB |
| `h264-2160p60.mp4`, `vah264dec` | 175.6 / 174.7 % | 36.3 / 41.0 | 95.0 / 44.8 | 140.8 / 235.5 MiB |

O que isso diz:

- **A CPU é a mesma.** Nas duas compilações é a conversão, o mesmo código.
- **O loop de eventos desperta cerca de metade das vezes.** As respostas às requisições de sincronização do `waylandsink` também iam para o loop de eventos do veil (entrada 6 acima); o novo sink não envia nenhuma.
- **A memória é maior neste compositor.** O sway headless retém o buffer que exibe até o próximo substituí-lo, então o sink precisa de três (`video sink: the compositor holds every buffer; using 3` em um log de depuração): cerca de 25 MiB a mais em 1080p, 100 MiB em 4K. Um compositor que copia um quadro para uma textura quando ele é confirmado pode deixar o sink ficar com dois. Não medido em uma sessão real.
- **Escalar na CPU não compensa.** Uma primeira versão convertia um quadro maior que o seu lugar no tamanho do lugar (4K nesta saída de 1080p: um quarto dos pixels). Custou mais e exibiu 33 quadros por segundo de 60: o conversor do GStreamer sai do seu caminho rápido quando escala. Medido com `gst-launch-1.0 ... nvh264dec ! videoconvertscale n-threads=2 ! ... ! fakesink sync=false`, 600 quadros de `h264-2160p60.mp4`: 2.8 s só decodificando, 11.4 s com a conversão em 4K, 15.9 s para 1080p com `method=bilinear`, 14.4 s com `4-tap`, 19.2 s com `lanczos`, 9.1 s com `nearest-neighbour`.
- **Uma primeira versão que não esvaziava a conexão depois de cada quadro exibia 3 quadros por segundo**: suas requisições esperavam no buffer da libwayland até o loop de eventos despertar por outro motivo. O `two_videos_play_side_by_side` (`tests/headless.rs`) agora verifica a taxa de quadros.

## Quadros em dmabufs (2026-11-20, sway headless na GPU Intel)

O sink recebe os quadros dos decodificadores VA-API em dmabufs e os entrega ao compositor como estão (`known-issues.md`, entrada 7). A mesma máquina de "O sink próprio do veil" acima (Intel Core i7-13620H com gráficos Raptor Lake, Mesa 26.3, e uma RTX 4050). O sway headless renderiza na GPU Intel (`bench.sh --renderer gles2 --render-device /dev/dri/renderD128`), então aceita dmabufs; os decodificadores da NVIDIA foram tirados do caminho (`GST_PLUGIN_FEATURE_RANK=nvh264dec:0,nvh265dec:0,nvav1dec:0,nvvp9dec:0,nvvp8dec:0`), então todo clipe foi decodificado por um decodificador VA-API. "Antes" é o commit `c5943ca`, que converte cada quadro na CPU; "depois" a primeira versão da mudança, que entregava todo vídeo ao compositor. Alternadas, `--duration 30`, `fill`, uma saída de 1920x1080, o alocador da glibc.

| Clipe, decodificador | CPU antes / depois | Quadros por s antes / depois (descartados pelo sink) | RSS antes / depois |
|---|---|---|---|
| `h264-1080p30.mp4`, `vah264dec` | 45.6 / 2.3 % | 30.0 (1) / 30.0 (0) | 144.6 / 121.4 MiB |
| `h264-1080p60.mp4`, `vah264dec` | 71.4 / 6.1 % | 60.0 (1) / 59.9 (0) | 143.9 / 121.4 MiB |
| `h264-2160p60.mp4`, `vah264dec` | 174.1 / 6.7 % | **39.5 (422)** / 60.0 (0) | 210.7 / 121.8 MiB |
| `hevc-2160p60.mp4`, `vah265dec` | 175.3 / 8.6 % | **41.0 (410)** / 60.0 (0) | 210.9 / 122.2 MiB |
| `vp9-1080p60.webm`, `vavp9dec` | 80.5 / 7.4 % | 59.5 (0) / 60.3 (0) | 140.2 / 120.0 MiB |
| `av1-1080p60.mkv`, `vaav1dec` | 82.7 / 7.9 % | 60.0 (0) / 59.6 (0) | 142.2 / 122.0 MiB |

No caminho da CPU (pixman, os decodificadores padrão: `nvh264dec` para H.264, `vavp9dec` para VP9) nada mudou: 44.6 / 43.6 % e 76.3 / 76.2 %, todos os quadros exibidos. As threads foram de 9 a 10 para 8 a 9, os descritores subiram de 34 para 43 a 46 (os dmabufs importados). O RSS subiu de 0.1 a 0.4 MiB por minuto em todas as execuções, antes e depois, que é o ruído de uma execução de 30 s.

A mudança final também verifica as cores (abaixo), e o `gles2` não pode ser informado delas: com a compilação final, `h264-1080p30.mp4` (sem marcação, por isso tomado como BT.709) é convertido na CPU ali, como antes. Com `--renderer vulkan`, que pode ser informado, a compilação final usou **3.4 %** contra 44.8 % antes (29.8 quadros por segundo, nenhum descartado, contra 26.5 com 104 descartados). Os outros clipes com `vulkan` não foram executados: a execução foi interrompida por falta de memória na máquina.

O que isso diz:

- **A conversão era todo o custo.** Sem ela, um vídeo 1080p custa de 2 a 8 % de um núcleo, cerca de um décimo a um vigésimo. O que resta é decodificar na GPU, o loop de eventos e o tratamento de cada quadro pelo sink.
- **4K a 60 fps é reproduzido por inteiro.** Antes, o caminho da CPU perdia um terço dos quadros a 175 % (suas duas threads de conversão não bastavam); agora, 60 por segundo a 7 a 9 %.
- **Menos memória.** Nenhum buffer `wl_shm`: os quadros ficam nos dmabufs do decodificador, cerca de 20 MiB a menos em 1080p e 90 MiB a menos em 4K.
- **As cores dependem de o compositor conhecer a codificação.** Com barras de cores codificadas como BT.709 e como BT.601 (agora um teste: `tests/gpu.rs`, [`testing.md`](https://github.com/ordreth/veil/blob/stable/docs/testing.md)), o `vulkan` exibiu as duas como o ffmpeg as decodifica, dentro de 3 em 255, depois que o veil lhe informou a codificação. O `gles2`, sem ser informado de nada, exibiu BT.601 certo e BT.709 errado (vermelho 231 em vez de 251, ciano com 20 de vermelho), e por isso um vídeo assim é convertido na CPU ali.

## Ainda não medido

- Uma sessão real (`--session`): ela precisa de uma sessão de desktop, que as execuções automatizadas não têm ([`testing.md`](https://github.com/ordreth/veil/blob/stable/docs/testing.md), "Not covered by any test"). Sabe-se que a conversão acontece ali também na NVIDIA (acima); o que resta é a parte do próprio compositor.
- GPUs AMD. O caminho VA-API da Intel está em "Quadros em dmabufs" acima, em um sway headless; não em uma sessão real.
- Um vídeo pausado e um liberado: os testes headless os verificam (CPU no máximo um quarto da de reprodução, e no máximo 2 ticks em 2 s), mas não há execução do bench para eles.
- Horas e dias: o teste de longa duração de 24 horas do checklist de lançamento ([`roadmap.md`](/apps/veil/docs/roadmap)), nesta máquina, também com o decodificador por software (a partida lenta do clipe de 1 s acima).
