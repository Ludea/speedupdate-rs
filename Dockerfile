FROM alpine:3.19
ARG TARGETARCH
RUN apk upgrade

WORKDIR /opt/speedupdate

COPY speedupdateserver ./speedupdateserver

EXPOSE 3000
EXPOSE 3001
EXPOSE 50051
EXPOSE 2121

CMD ["./speedupdateserver"] 
