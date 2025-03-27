FROM alpine:3.21
ARG TARGETARCH
RUN apk upgrade

WORKDIR /opt/speedupdate

COPY speedupdate-$TARGETARCH/speedupdate /usr/local/bin/speedupdate
COPY speedupdate-$TARGETARCH/speedupdateserver .

RUN chmod +x speedupdateserver
RUN chmod +x /usr/local/bin/speedupdate

COPY pkey .
EXPOSE 3000
EXPOSE 3001
EXPOSE 8080
EXPOSE 2121

CMD ["./speedupdateserver"] 
