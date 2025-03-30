FROM alpine:3.21
ARG TARGETARCH
RUN apk upgrade

WORKDIR /opt/speedupdate

COPY speedupdate-$TARGETARCH/speedupdate /usr/local/bin/speedupdate
COPY speedupdate-$TARGETARCH/speedupdateserver /usr/local/bin/speedupdateserver

RUN chmod +x /usr/local/bin/speedupdateserver
RUN chmod +x /usr/local/bin/speedupdate

COPY pkey .

EXPOSE 8012

CMD ["speedupdateserver"] 
