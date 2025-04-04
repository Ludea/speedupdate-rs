FROM alpine:3.21
ARG TARGETARCH
RUN apk upgrade

WORKDIR /opt/speedupdate

COPY speedupdate-$TARGETARCH/speedupdate /usr/local/bin/foo/speedupdate
COPY speedupdate-$TARGETARCH/speedupdateserver /usr/local/bin/foo/speedupdateserver

RUN chmod +x /usr/local/bin/foo/speedupdateserver
RUN chmod +x /usr/local/bin/foo/speedupdate

RUN ln -s /usr/local/bin/foo/speedupdate/usr/local/bin/speedupdate
RUN ln -s /usr/local/bin/foo/speedupdateserver /usr/local/bin/speedupdateserver

COPY pkey /usr/local/bin/speedupdate/

EXPOSE 8012

CMD ["speedupdateserver"] 
