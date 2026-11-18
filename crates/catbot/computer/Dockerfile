# A bot's own computer (stage 17.1): a small Debian desktop with a browser, seen and used
# through noVNC. Catbot builds it on first use; nothing of the person's is put inside.
FROM debian:bookworm-slim
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      xvfb x11vnc novnc websockify fluxbox xdotool imagemagick chromium \
      ca-certificates curl git python3 sudo procps fonts-dejavu-core xterm \
    && rm -rf /var/lib/apt/lists/*
# The bot's own account: it may do anything inside its box, sudo included.
RUN useradd -m -s /bin/bash bot && echo 'bot ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/bot \
    && chmod 0440 /etc/sudoers.d/bot \
    && mkdir -p /home/bot/.fluxbox \
    && printf 'session.screen0.toolbar.visible: true\nsession.styleFile: /usr/share/fluxbox/styles/Emerge\nsession.styleOverlay: ~/.fluxbox/overlay\n' > /home/bot/.fluxbox/init \
    && printf 'background: solid\nbackground.color: #2e3440\n' > /home/bot/.fluxbox/overlay \
    && chown -R bot:bot /home/bot
COPY start.sh /usr/local/bin/catbot-computer
RUN chmod 0755 /usr/local/bin/catbot-computer
USER bot
WORKDIR /home/bot
ENV DISPLAY=:1 HOME=/home/bot
EXPOSE 6080
CMD ["/usr/local/bin/catbot-computer"]
