FROM alpine:3.21
ARG TARGETARCH
RUN apk upgrade

WORKDIR /opt/speedupdate

COPY speedupdate-$TARGETARCH/speedupdate /usr/local/bin/speedupdate/speedupdate
COPY speedupdate-$TARGETARCH/speedupdateserver /usr/local/bin/speedupdate/speedupdateserver

RUN chmod +x /usr/local/bin/speedupdateserver
RUN chmod +x /usr/local/bin/speedupdate

RUN ls -s /usr/local/bin/speedupdate/speedupdate /usr/local/bin/speedupdate
RUN ls -s /usr/local/bin/speedupdate/speedupdateserver /usr/local/bin/speedupdateserver

COPY pkey /usr/local/bin/speedupdate/

EXPOSE 8012

CMD ["speedupdateserver"] 
