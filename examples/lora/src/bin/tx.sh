#!/bin/bash

BUILD_TYPE=""
#BUILD_TYPE="--release"
CENTER_FREQ="--center-freq=867000000"
#RX_OFFSET="--rx-freq-offset=1.1e6"
TX_OFFSET="--tx-freq-offset=1.1e6"
#DEVICE_FILTER="--device-filter=driver=aaronia_http,tx_url=http://172.18.0.1:54665,url=http://172.18.0.1:54664"
#RX_GAIN="--rx-gain 10"
TX_DEVICE_FILTER="--device-filter=driver=soapy,soapy_driver=uhd,type=b200,name=MyB210"
TX_GAIN="--tx-gain 40"
#RX_ANTENNA=""
TX_ANTENNA="--tx-antenna=1"
SAMPLE_RATE="--sample-rate 125000"
TX_INTERVAL="--tx-interval 3"
SPREADING_FACTOR="--spreading-factor 8"
BANDWIDTH="--bandwidth 125000"

export FUTURESDR_LOG_LEVEL=debug
export RUST_BACKTRACE=full
#export FUTURESDR_LOG_LEVEL=warn
#export FUTURESDR_CTRLPORT_ENABLE=true
#export FUTURESDR_CTRLPORT_BIND="0.0.0.0:1348"

cargo run --bin tx ${BUILD_TYPE} -- ${SPREADING_FACTOR} ${BANDWIDTH} ${TX_INTERVAL} ${CENTER_FREQ} ${TX_OFFSET} ${TX_DEVICE_FILTER} ${TX_GAIN} ${TX_ANTENNA} ${SAMPLE_RATE}

