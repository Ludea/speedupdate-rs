FROM alpine:3.20
ARG TARGETARCH
RUN apk upgrade

WORKDIR /opt/speedupdate

COPY speedupdate-$TARGETARCH/speedupdate /usr/local/bin/speedupdate
COPY speedupdate-$TARGETARCH/speedupdateserver .

RUN chmod +x speedupdateserver
RUN chmod +x /usr/local/bin/speedupdate

EXPOSE 3000
EXPOSE 3001
EXPOSE 50051
EXPOSE 2121

CMD ["./speedupdateserver"] 
