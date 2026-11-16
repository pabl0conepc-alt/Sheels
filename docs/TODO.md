# Pendências (relatadas no uso real, 2026-11-16)

- [x] CPU alta ao abrir a dashboard: a janela era reconstruída a cada abertura (agora é persistente) e o clima tentava geolocalizar de novo a cada abertura quando falhava.
- [ ] Animações/hover mais rápidos (transições de 200/400 ms, abrir janela ao passar o mouse leva um frame extra).
- [ ] Sessão: arrastar o painel para o lado para revelar a sidebar (notificações + IA), como no Caelestia.
- [ ] Foto do usuário: clicar no avatar da dashboard para escolher a imagem (hoje: copiar para `~/.face`).
- [ ] Atalho de teclado para abrir o Nexus (configurações).
- [ ] Revisar paridade com a versão mais recente do Caelestia (Hyprland).

## Paridade com o Caelestia mais recente (o que ainda falta)

Ligados por padrão no original:
- [ ] Aba Mídia: letras sincronizadas (LyricsAndSelector) e visualizador na capa (CoverVisualiser, cava).
- [ ] Barra: popout de layout de teclado (lista e troca de layouts), menu da bandeja no estilo Caelestia (TrayMenu), ícone da distro (OsIcon) quando configurado.
- [ ] Info de janela (windowinfo): prévia da janela com botões (hoje o popout mostra só detalhes e ações básicas).
- [ ] Utilitários: confirmação ao apagar gravação (RecordingDeleteModal).

Desligados por padrão no original:
- [ ] Fundo: relógio na área de trabalho (DesktopClock) e visualizador (Visualiser).
- [ ] Special workspaces (o driftwm não tem).
