FROM rust:1.95

# Match a typical host Linux UID/GID (override in devcontainer.json "build" -> "args" if needed)
ARG USERNAME=vscode
ARG USER_UID=1000
ARG USER_GID=1000

ENV RUSTUP_HOME=/usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo

RUN apt-get update && apt-get install -y --no-install-recommends \
		sudo \
		python3 \
		python3-pip \
		python3-matplotlib \
	&& groupadd --gid $USER_GID $USERNAME \
	&& useradd -s /bin/bash -m -u $USER_UID -g $USER_GID $USERNAME \
	&& chown -R $USER_UID:$USER_GID $RUSTUP_HOME $CARGO_HOME \
	&& echo "$USERNAME ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/$USERNAME \
	&& chmod 440 /etc/sudoers.d/$USERNAME \
	&& rm -rf /var/lib/apt/lists/*

USER $USERNAME
