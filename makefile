TARGET = robustmq
VERSION = v0.0.1
BUILD_FOLD = ./build
PACKAGE_FOLD_NAME = ${TARGET}-$(VERSION)-release
all:
	cargo build

release:
	mkdir -p ${BUILD_FOLD}
	cargo build --release
	cd src/cmd && cargo build --release && cd ../../
	mkdir -p $(BUILD_FOLD)/${PACKAGE_FOLD_NAME}
	mkdir -p $(BUILD_FOLD)/${PACKAGE_FOLD_NAME}/bin
	mkdir -p $(BUILD_FOLD)/${PACKAGE_FOLD_NAME}/libs
	mkdir -p $(BUILD_FOLD)/${PACKAGE_FOLD_NAME}/config
	cp -rf target/release/robustmq $(BUILD_FOLD)/${PACKAGE_FOLD_NAME}/libs 
	cp -rf src/cmd/target/release/robustmq-cmd $(BUILD_FOLD)/${PACKAGE_FOLD_NAME}/libs 
	cp -rf bin/* $(BUILD_FOLD)/${PACKAGE_FOLD_NAME}/bin
	cp -rf config/* $(BUILD_FOLD)/${PACKAGE_FOLD_NAME}/config
	cd $(BUILD_FOLD) && tar zcvf ${PACKAGE_FOLD_NAME}.tar.gz ${PACKAGE_FOLD_NAME}
	
	echo "build release package success. ${PACKAGE_FOLD_NAME}.tar.gz "

clean:
	cargo clean
	cd src/cmd && cargo clean && cd ../../
	rm -rf build