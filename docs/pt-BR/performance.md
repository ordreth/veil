---
title: "Desempenho"
description: "CPU, memória e taxa de quadros medidas por tipo de papel de parede."
group: "Reference"
order: 6
source: "README.md"
source_hash: "5975e5f07fe4fc374c8f3b3a67e9ff1a73f895d7bd2f17ad007e2f368768d6fa"
---

Medido com [`scripts/bench/`](https://github.com/ordreth/veil/tree/main/scripts/bench) na compilação de 2026-10-19, em um sway 1.12 headless que renderiza na CPU, com uma saída de 1920x1080 em `fill`, 60 segundos após um aquecimento de 5 segundos, com `MALLOC_ARENA_MAX=1`, como o pacote e a unidade o iniciam. Os clipes são padrões de teste do ffmpeg em que todo quadro é diferente. A máquina: Intel Core i5-11400F, NVIDIA GeForce RTX 3060 Ti (driver 595.91.07, por isso o GStreamer escolhe os decodificadores `nvcodec`), 32 GB, NixOS com Linux 7.2 e GStreamer 1.28.7. A CPU é dada em porcentagem de um núcleo, para o processo inteiro do daemon.

| Papel de parede | Decodificador | CPU | Quadros exibidos por segundo | Memória (RSS) | Threads |
|---|---|---|---|---|---|
| Um PNG de 1920x1080 | nenhum | 0.02 % (um tick de 10 ms em 60 s) | | 20 MiB | 1 |
| H.264, 1080p, 30 fps | `nvh264dec` (GPU) | 19.5 % | 30.0 de 30 | 203 MiB | 15 |
| H.264, 1080p, 60 fps | `nvh264dec` (GPU) | 39.7 % | 59.8 de 60 | 203 MiB | 15 |
| VP9, 1080p, 60 fps | `nvvp9dec` (GPU) | 37.9 % | 59.9 de 60 | 251 MiB | 15 |
| H.264, 2160p, 60 fps | `nvh264dec` (GPU) | 136 % | 60.2 de 60 | 297 MiB | 15 |
| H.264, 1080p, 30 fps | `avdec_h264` (software) | 24.5 % | 30.0 de 30 | 161 MiB | 22 |
| H.264, 2160p, 60 fps | `avdec_h264` (software) | 174 % | 60.1 de 60 | 458 MiB | 22 |

- **Uma imagem estática não custa quase nada** depois de desenhada: uma thread e um tick de tempo de CPU em um minuto, enquanto o bench perguntava `veil status` a cada segundo.
- **Um vídeo custa CPU em proporção aos seus pixels por segundo**, seja qual for o codec. O decodificador na GPU custa cerca de 1 %; o resto é a conversão de cada quadro de NV12 para RGB, que o compositor exige e que acontece na CPU com os decodificadores da NVIDIA. A meta de projeto do veil, de menos de 1 % para um vídeo, **não** é atingida neste hardware. Em Intel e AMD, o VA-API poderia entregar os quadros ao compositor sem essa conversão; isso não foi medido ([problema conhecido 7](/apps/veil/docs/known-issues)).
- **Memória, threads e descritores de arquivo permanecem estáveis**: um vídeo que repetiu uma vez por segundo durante dez minutos não reteve nada (com o decodificador por hardware; o decodificador por software reteve cerca de 500 KiB nos primeiros minutos, depois nada), e 500 trocas de papel de parede (250 delas vídeos) não deixaram nada para trás. A memória só permanece estável ao longo das trocas com uma única arena do alocador (`MALLOC_ARENA_MAX=1`), que o pacote Nix e a unidade do systemd definem; o [problema conhecido 3](/apps/veil/docs/known-issues) traz os números sem ela. Execuções de horas e de dias ainda estão por fazer.
- **Um vídeo pausado ou parado usa quase nenhuma CPU**: atrás de uma janela em tela cheia, ele mantém seu quadro na tela e não decodifica nada (medido: no máximo um tick em 5 segundos); enquanto a tela está bloqueada, seu pipeline é liberado.

Como cada número foi medido, e o que foi medido antes, está em [`docs/measurements.md`](/apps/veil/docs/measurements). Nada foi medido ainda em uma sessão real.
